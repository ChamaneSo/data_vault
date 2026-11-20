"""Run only against the disposable, size-limited fault stack."""
import os
import uuid

import pytest
from botocore.exceptions import ClientError

from client import account, admin, s3


@pytest.fixture
def isolated_bucket():
    assert os.environ["S3_ENDPOINT"] == "http://fault-server:9000"
    user, credentials = account(True)
    store = s3(credentials)
    bucket = "fault-" + uuid.uuid4().hex
    store.create_bucket(Bucket=bucket)
    try:
        yield store, bucket
    finally:
        for page in store.get_paginator("list_objects_v2").paginate(Bucket=bucket):
            for item in page.get("Contents", []):
                store.delete_object(Bucket=bucket, Key=item["Key"])
        store.delete_bucket(Bucket=bucket)
        admin("DELETE", "service-accounts/" + user)


def test_deferred_commit_failure_preserves_previous_version(isolated_bucket):
    store, bucket = isolated_bucket
    key = "commit-failure"
    before = store.put_object(Bucket=bucket, Key=key, Body=b"original")
    with pytest.raises(ClientError) as error:
        store.put_object(Bucket=bucket, Key=key, Body=b"uncommitted", Metadata={"test": "reject"})
    assert error.value.response["ResponseMetadata"]["HTTPStatusCode"] >= 500
    read = store.get_object(Bucket=bucket, Key=key)
    assert read["ETag"] == before["ETag"]
    assert read["Body"].read() == b"original"


def test_full_scratch_volume_preserves_acknowledged_data(isolated_bucket, tmp_path):
    store, bucket = isolated_bucket
    store.put_object(Bucket=bucket, Key="capacity", Body=b"original")
    path = tmp_path / "larger-than-scratch"
    with path.open("wb") as data:
        data.truncate(20 * 1024 * 1024)
    with path.open("rb") as data, pytest.raises(ClientError) as error:
        store.put_object(Bucket=bucket, Key="capacity", Body=data)
    assert error.value.response["ResponseMetadata"]["HTTPStatusCode"] >= 500
    assert store.get_object(Bucket=bucket, Key="capacity")["Body"].read() == b"original"
    store.put_object(Bucket=bucket, Key="after-failure", Body=b"writable")
