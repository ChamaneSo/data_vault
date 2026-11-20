"""Local DataVault browser. Each browser session signs as its own S3 service account."""
import json
import os
import secrets
import shutil
import tempfile
import threading
import time
from collections.abc import Mapping
from urllib.error import URLError
from urllib.parse import quote
from urllib.request import urlopen

import boto3
from botocore.config import Config
from botocore.exceptions import BotoCoreError, ClientError
from flask import Flask, Response, jsonify, render_template, request
from werkzeug.exceptions import HTTPException

app = Flask(__name__)
UPLOAD_LIMIT = 128 * 1024 * 1024
COOKIE = "datavault_browser"
API = "/web-api"
SESSION_SECONDS = 3600
S3_ENDPOINT = os.getenv("S3_ENDPOINT", "http://datavault:9000")
ADMIN_ENDPOINT = os.getenv("ADMIN_ENDPOINT", "http://datavault:9001")
REGION = os.getenv("AWS_DEFAULT_REGION", "us-east-1")
app.config.update(MAX_CONTENT_LENGTH=UPLOAD_LIMIT, TRUSTED_HOSTS=["localhost", "127.0.0.1", "[::1]", "web"])

# One threaded worker owns the sessions. In-flight requests retain their client when a session ends.
sessions = {}
session_lock = threading.Lock()


class Problem(Exception):
    def __init__(self, status, message):
        self.status = status
        self.message = message


@app.errorhandler(Problem)
def problem(error):
    return jsonify(error=error.message), error.status


@app.errorhandler(HTTPException)
def http_error(error):
    message = "Files must be 128 MiB or smaller." if error.code == 413 else error.name
    return jsonify(error=message), error.code


@app.errorhandler(BotoCoreError)
def transport_error(_error):
    return jsonify(error="Could not complete the request to DataVault."), 502


@app.errorhandler(ClientError)
def service_error(error):
    code = error.response.get("Error", {}).get("Code", "")
    messages = {
        "AccessDenied": (403, "This account does not have permission for that action."),
        "InvalidAccessKeyId": (401, "The access key is invalid or has been revoked."),
        "SignatureDoesNotMatch": (401, "DataVault could not verify the signing credentials."),
        "NoSuchBucket": (404, "This bucket no longer exists."),
        "NoSuchKey": (404, "This object no longer exists."),
        "404": (404, "The requested object or bucket no longer exists."),
        "403": (403, "This account does not have permission for that action."),
        "BucketAlreadyExists": (409, "That bucket name is already in use."),
        "BucketAlreadyOwnedByYou": (409, "You already own a bucket with that name."),
        "PreconditionFailed": (409, "An object already uses this key. Choose another key or enable replacement."),
        "InvalidBucketName": (400, "Enter a valid lowercase bucket name."),
        "InvalidArgument": (400, "DataVault rejected a request parameter."),
    }
    status, message = messages.get(code, (502, "DataVault could not complete the operation."))
    if status == 401:
        with session_lock:
            sessions.pop(request.cookies.get(COOKIE), None)
    return jsonify(error=message), status


@app.before_request
def protect_writes():
    if request.method in {"POST", "PUT", "DELETE"}:
        origin = request.headers.get("Origin")
        if request.headers.get("X-DataVault-Request") != "1" or (
            origin and origin != request.host_url.rstrip("/")
        ):
            raise Problem(403, "Use the DataVault browser to make this request.")
        if request.method == "POST":
            request.max_content_length = 4096


@app.after_request
def security_headers(response):
    response.headers.update({
        "Cache-Control": "no-store",
        "X-Content-Type-Options": "nosniff",
        "Referrer-Policy": "no-referrer",
        "X-Frame-Options": "DENY",
        "Content-Security-Policy": "default-src 'self'; script-src 'self'; style-src 'self'; "
                                   "img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; "
                                   "object-src 'none'; base-uri 'none'; form-action 'self'",
    })
    return response


def session_info():
    now = time.monotonic()
    with session_lock:
        for token, (_, _, deadline) in list(sessions.items()):
            if deadline <= now:
                del sessions[token]
        return sessions.get(request.cookies.get(COOKIE))


def client():
    current = session_info()
    if not current:
        raise Problem(401, "Connect your service account to continue.")
    return current[0]


def field(data, name, limit):
    value = data.get(name) if isinstance(data, Mapping) else None
    if not isinstance(value, str) or not value or len(value.encode("utf-8")) > limit:
        raise Problem(400, f"Invalid {name.replace('_', ' ')}.")
    return value


@app.get("/")
def index():
    return render_template("index.html")


@app.get(API + "/health")
def health():
    try:
        with urlopen(ADMIN_ENDPOINT + "/health/ready", timeout=3) as response:
            ready = json.load(response).get("ready") is True
    except (OSError, URLError, ValueError):
        ready = False
    return jsonify(ready=ready)


@app.get(API + "/session")
def session_status():
    current = session_info()
    return jsonify(account=current[1] if current else None, region=REGION)


@app.post(API + "/session")
def connect():
    data = request.get_json()
    access_key = field(data, "access_key", 128)
    secret_key = field(data, "secret_key", 256)
    s3 = boto3.session.Session().client(
        "s3", endpoint_url=S3_ENDPOINT, region_name=REGION,
        aws_access_key_id=access_key, aws_secret_access_key=secret_key,
        config=Config(signature_version="s3v4", s3={"addressing_style": "path"},
                      request_checksum_calculation="when_required", retries={"max_attempts": 1},
                      connect_timeout=4, read_timeout=90),
    )
    s3.list_buckets(MaxBuckets=1)
    session_info()
    token = secrets.token_urlsafe(32)
    label = "•••• " + access_key[-4:]
    with session_lock:
        sessions.pop(request.cookies.get(COOKIE), None)
        if len(sessions) >= 128:
            raise Problem(429, "The session limit has been reached. Try again later.")
        sessions[token] = (s3, label, time.monotonic() + SESSION_SECONDS)
    response = jsonify(account=label, region=REGION)
    response.set_cookie(COOKIE, token, max_age=SESSION_SECONDS, httponly=True, samesite="Strict", path=API)
    return response


@app.delete(API + "/session")
def disconnect():
    with session_lock:
        sessions.pop(request.cookies.get(COOKIE), None)
    response = Response(status=204)
    response.delete_cookie(COOKIE, path=API, httponly=True, samesite="Strict")
    return response


@app.get(API + "/buckets")
def buckets():
    options = {"MaxBuckets": 100}
    if token := request.args.get("cursor"):
        options["ContinuationToken"] = token
    page = client().list_buckets(**options)
    return jsonify(buckets=[item["Name"] for item in page.get("Buckets", [])],
                   next=page.get("ContinuationToken"))


@app.post(API + "/buckets")
def create_bucket():
    s3 = client()
    name = field(request.get_json(), "name", 63)
    s3.create_bucket(Bucket=name)
    return jsonify(name=name), 201


@app.get(API + "/buckets/<bucket>/objects")
def objects(bucket):
    prefix = request.args.get("prefix", "")
    options = {"Bucket": bucket, "Prefix": prefix, "MaxKeys": 50}
    if request.args.get("flat") != "1":
        options["Delimiter"] = "/"
    if token := request.args.get("cursor"):
        options["ContinuationToken"] = token
    page = client().list_objects_v2(**options)
    return jsonify(folders=[item["Prefix"] for item in page.get("CommonPrefixes", [])],
                   objects=[{"key": obj["Key"], "size": obj["Size"],
                             "modified": obj["LastModified"].isoformat()}
                            for obj in page.get("Contents", [])],
                   next=page.get("NextContinuationToken"))


@app.put(API + "/buckets/<bucket>/object")
def upload(bucket):
    s3 = client()
    key = field(request.args, "key", 1024)
    options = {"Bucket": bucket, "Key": key, "ContentType": request.mimetype or "application/octet-stream"}
    if request.args.get("replace") != "1":
        options["IfNoneMatch"] = "*"
    # The SDK needs a seekable body for signing. Stage on disk and bound reads.
    with tempfile.TemporaryFile() as body:
        shutil.copyfileobj(request.stream, body, length=64 * 1024)
        options["ContentLength"] = body.tell()
        body.seek(0)
        s3.put_object(Body=body, **options)
    return jsonify(key=key), 201


@app.get(API + "/buckets/<bucket>/download")
def download(bucket):
    key = field(request.args, "key", 1024)
    s3 = client()
    if request.method == "HEAD":
        metadata = s3.head_object(Bucket=bucket, Key=key)
        return Response(headers={"Content-Length": str(metadata["ContentLength"])})
    obj = s3.get_object(Bucket=bucket, Key=key)
    body = obj["Body"]
    filename = (key.rsplit("/", 1)[-1] or "object").replace("\\", "_")
    response = Response(body.iter_chunks(64 * 1024), mimetype="application/octet-stream")
    response.content_length = obj["ContentLength"]
    response.headers["Content-Disposition"] = "attachment; filename*=UTF-8''" + quote(filename, safe="")
    response.call_on_close(body.close)
    return response
