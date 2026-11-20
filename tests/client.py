"""Real HTTP clients shared by the integration suite and restart demonstration."""
import json
import os
import urllib.request
import uuid

import boto3
from botocore.config import Config

ADMIN = os.environ["ADMIN_ENDPOINT"]
S3 = os.environ["S3_ENDPOINT"]
REGION = os.environ.get("AWS_DEFAULT_REGION", "us-east-1")


class Credentials(dict):
    """Keep one-time secrets out of pytest fixture representations."""
    def __repr__(self):
        return "<DataVault test credentials>"


def admin(method, path, payload=None, token=None):
    secret = os.environ["DATAVAULT_ADMIN_TOKEN"] if token is None else token
    headers = {"Authorization": f"Bearer {secret}", "Content-Type": "application/json"}
    body = json.dumps(payload).encode() if payload is not None else None
    request = urllib.request.Request(ADMIN + "/api/v1/" + path, data=body, headers=headers, method=method)
    with urllib.request.urlopen(request, timeout=15) as response:
        return json.load(response)


def account(create_buckets=False):
    user = admin("POST", "service-accounts", {
        "name": "test-" + uuid.uuid4().hex,
        "can_create_buckets": create_buckets,
    })
    key = admin("POST", f"service-accounts/{user['id']}/access-keys")
    return user["id"], Credentials(key)


def s3(key):
    return boto3.client("s3", endpoint_url=S3, region_name=REGION,
                        aws_access_key_id=key["access_key"], aws_secret_access_key=key["secret_key"],
                        config=Config(signature_version="s3v4", s3={"addressing_style": "path"},
                                      retries={"max_attempts": 0}))
