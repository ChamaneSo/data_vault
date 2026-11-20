import concurrent.futures
import datetime
import hashlib
import os
import subprocess
import urllib.error
import urllib.request
import uuid
from types import SimpleNamespace
from unittest.mock import patch

import pytest
from botocore.exceptions import ClientError

from client import ADMIN, REGION, S3, account, admin, s3


@pytest.fixture
def owner():
    user, key = account(True)
    client = s3(key)
    bucket = "dv-" + uuid.uuid4().hex
    client.create_bucket(Bucket=bucket)
    try:
        yield client, bucket, key
    finally:
        for page in client.get_paginator("list_objects_v2").paginate(Bucket=bucket):
            for obj in page.get("Contents", []):
                client.delete_object(Bucket=bucket, Key=obj["Key"])
        client.delete_bucket(Bucket=bucket)
        admin("DELETE", "service-accounts/" + user)


def rejected(call, code):
    with pytest.raises(ClientError) as caught:
        call()
    assert caught.value.response["Error"]["Code"] == code


def test_object_contract(owner):
    c, bucket, _ = owner
    data = bytes(range(256)) * 513
    key = "raw/space + café.bin"
    c.head_bucket(Bucket=bucket)
    assert bucket in [b["Name"] for b in c.list_buckets()["Buckets"]]
    put = c.put_object(Bucket=bucket, Key=key, Body=data, ContentType="application/x-test",
                       Metadata={"format": "binary"})
    head = c.head_object(Bucket=bucket, Key=key)
    assert head["Metadata"] == {"format": "binary"}
    assert head["ContentLength"] == len(data)
    assert head["ContentType"] == "application/x-test"
    assert head["ETag"] == put["ETag"]
    assert c.get_object(Bucket=bucket, Key=key)["Body"].read() == data
    part = c.get_object(Bucket=bucket, Key=key, Range="bytes=4-20", IfMatch=put["ETag"])
    assert part["ResponseMetadata"]["HTTPStatusCode"] == 206
    assert part["Body"].read() == data[4:21]
    rejected(lambda: c.put_object(Bucket=bucket, Key=key, Body=b"x", IfNoneMatch="*"), "PreconditionFailed")
    rejected(lambda: c.delete_bucket(Bucket=bucket), "BucketNotEmpty")
    c.copy_object(Bucket=bucket, Key="copy", CopySource={"Bucket": bucket, "Key": key})
    assert c.get_object(Bucket=bucket, Key="copy")["Body"].read() == data
    c.put_object(Bucket=bucket, Key=key, Body=b"")
    assert c.get_object(Bucket=bucket, Key=key)["Body"].read() == b""
    c.delete_object(Bucket=bucket, Key=key)
    rejected(lambda: c.get_object(Bucket=bucket, Key=key), "NoSuchKey")


def test_listing_and_concurrent_overwrites(owner):
    c, bucket, _ = owner
    keys = ["a/01", "a/02", "a/b/03", "a/b/04", "a/c/05", "a_%", "q"]
    for key in keys:
        c.put_object(Bucket=bucket, Key=key, Body=b"")
    pages = list(c.get_paginator("list_objects_v2").paginate(
        Bucket=bucket, Prefix="a/", PaginationConfig={"PageSize": 1}))
    assert [o["Key"] for page in pages for o in page.get("Contents", [])] == keys[:5]
    grouped = list(c.get_paginator("list_objects_v2").paginate(
        Bucket=bucket, Prefix="a/", Delimiter="/", PaginationConfig={"PageSize": 1}))
    assert [p["Prefix"] for page in grouped for p in page.get("CommonPrefixes", [])] == ["a/b/", "a/c/"]
    assert [o["Key"] for o in c.list_objects_v2(Bucket=bucket, Prefix="a_%").get("Contents", [])] == ["a_%"]
    payloads = [bytes([i]) * (256 * 1024 + i) for i in range(8)]
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        list(pool.map(lambda b: c.put_object(Bucket=bucket, Key="race", Body=b), payloads))
    result = c.get_object(Bucket=bucket, Key="race")
    data = result["Body"].read()
    assert data in payloads and result["ContentLength"] == len(data)
    assert result["ETag"] == '"' + hashlib.md5(data).hexdigest() + '"'


def test_permissions_and_revocation(owner):
    writer, bucket, _ = owner
    writer.put_object(Bucket=bucket, Key="only", Body=b"visible")
    uid, key = account()
    reader = s3(key)
    try:
        rejected(lambda: reader.list_objects_v2(Bucket=bucket), "AccessDenied")
        rejected(lambda: reader.create_bucket(Bucket="dv-" + uuid.uuid4().hex), "AccessDenied")
        binding = admin("POST", "permissions", {"account_id": uid, "bucket": bucket, "role": "ReadOnly"})
        assert reader.get_object(Bucket=bucket, Key="only")["Body"].read() == b"visible"
        rejected(lambda: reader.put_object(Bucket=bucket, Key="bad", Body=b""), "AccessDenied")
        admin("DELETE", "permissions/" + binding["id"])
        rejected(lambda: reader.list_objects_v2(Bucket=bucket), "AccessDenied")
        keys = admin("GET", f"service-accounts/{uid}/access-keys")
        assert all("secret_key" not in info and "encrypted_secret" not in info for info in keys)
        admin("DELETE", "access-keys/" + key["id"])
        rejected(reader.list_buckets, "InvalidAccessKeyId")
    finally:
        admin("DELETE", "service-accounts/" + uid)


def test_admin_auth_and_health():
    with pytest.raises(urllib.error.HTTPError) as denied:
        admin("GET", "server/info", token="invalid")
    assert denied.value.code == 401
    for path in ["/health/live", "/health/ready", "/metrics"]:
        with urllib.request.urlopen(ADMIN + path, timeout=5) as response:
            assert response.status == 200


def test_integrity_and_signed_urls(owner):
    c, bucket, keys = owner
    rejected(lambda: s3({**keys, "secret_key": "wrong"}).list_buckets(), "SignatureDoesNotMatch")
    rejected(lambda: c.put_object(Bucket=bucket, Key="bad", Body=b"payload",
                                  ContentMD5="AAAAAAAAAAAAAAAAAAAAAA=="), "BadDigest")
    rejected(lambda: c.get_object(Bucket=bucket, Key="bad"), "NoSuchKey")
    c.put_object(Bucket=bucket, Key="signed", Body=b"signed content")
    url = c.generate_presigned_url("get_object", Params={"Bucket": bucket, "Key": "signed"}, ExpiresIn=60)
    with urllib.request.urlopen(url, timeout=10) as response:
        assert response.read() == b"signed content"


def test_disabled_account_loses_all_active_keys():
    uid, first = account()
    second = admin("POST", f"service-accounts/{uid}/access-keys")
    assert s3(first).list_buckets()["ResponseMetadata"]["HTTPStatusCode"] == 200
    admin("DELETE", "service-accounts/" + uid)
    for key in [first, second]:
        rejected(s3(key).list_buckets, "InvalidAccessKeyId")


def test_correct_signature_with_stale_timestamp_is_rejected(owner):
    client, _, _ = owner

    class StaleClock(datetime.datetime):
        @classmethod
        def utcnow(cls):
            return datetime.datetime.now(datetime.timezone.utc).replace(tzinfo=None) - datetime.timedelta(hours=2)

    with patch("botocore.auth.datetime", SimpleNamespace(datetime=StaleClock)):
        rejected(client.list_buckets, "RequestTimeTooSkewed")


def test_multipart(owner):
    c, bucket, _ = owner
    upload = c.create_multipart_upload(Bucket=bucket, Key="assembled")["UploadId"]
    args = {"Bucket": bucket, "Key": "assembled", "UploadId": upload}
    blocks = [b"a" * (5 * 1024 * 1024), b"last"]
    parts = []
    for number, data in enumerate(blocks, 1):
        result = c.upload_part(**args, PartNumber=number, Body=data)
        parts.append({"PartNumber": number, "ETag": result["ETag"]})
    assert [part["PartNumber"] for part in c.list_parts(**args)["Parts"]] == [1, 2]
    rejected(lambda: c.get_object(Bucket=bucket, Key="assembled"), "NoSuchKey")
    rejected(lambda: c.complete_multipart_upload(**args, MultipartUpload={"Parts": parts[::-1]}), "InvalidPartOrder")
    third = c.upload_part(**args, PartNumber=3, Body=b"tail")
    small_parts = [parts[1], {"PartNumber": 3, "ETag": third["ETag"]}]
    rejected(lambda: c.complete_multipart_upload(**args, MultipartUpload={"Parts": small_parts}), "EntityTooSmall")
    result = c.complete_multipart_upload(**args, MultipartUpload={"Parts": parts})
    assert result["ETag"].endswith('-2"')
    assert c.get_object(Bucket=bucket, Key="assembled")["Body"].read() == b"".join(blocks)
    args["UploadId"] = c.create_multipart_upload(Bucket=bucket, Key="assembled")["UploadId"]
    c.abort_multipart_upload(**args)
    rejected(lambda: c.list_parts(**args), "NoSuchUpload")


def test_aws_cli(owner, tmp_path):
    _, bucket, key = owner
    env = {**os.environ, "AWS_ACCESS_KEY_ID": key["access_key"], "AWS_SECRET_ACCESS_KEY": key["secret_key"],
           "AWS_DEFAULT_REGION": REGION, "AWS_PAGER": ""}
    def aws(*args):
        return subprocess.run(["aws", "--endpoint-url", S3, *args], check=True, env=env,
                              capture_output=True, timeout=60).stdout
    src = tmp_path / "hello"
    dst = tmp_path / "download"
    src.write_bytes(b"Hello DataVault\n")
    aws("s3", "cp", str(src), f"s3://{bucket}/hello")
    assert b"hello" in aws("s3", "ls", f"s3://{bucket}/")
    aws("s3", "cp", f"s3://{bucket}/hello", str(dst))
    assert hashlib.sha256(dst.read_bytes()).digest() == hashlib.sha256(src.read_bytes()).digest()
