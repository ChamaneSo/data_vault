"""Two-phase AWS CLI demonstration across a user-controlled server restart."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import uuid

from client import REGION, S3, account, admin

state = Path("/state/restart-demo.json")
parser = argparse.ArgumentParser()
parser.add_argument("phase", choices=["write", "verify"])
phase = parser.parse_args().phase
if phase == "write":
    if state.exists():
        raise SystemExit("A demo is pending. Run the verify phase before starting another.")
    user, key = account(True)
    record = {"user": user, "key": key, "bucket": "datavault-demo-" + uuid.uuid4().hex[:10]}
    with os.fdopen(os.open(state, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as f:
        json.dump(record, f)
else:
    record = json.loads(state.read_text())
env = {**os.environ, "AWS_ACCESS_KEY_ID": record["key"]["access_key"],
       "AWS_SECRET_ACCESS_KEY": record["key"]["secret_key"], "AWS_PAGER": "", "AWS_DEFAULT_REGION": REGION}
def aws(*args):
    subprocess.run(["aws", "--endpoint-url", S3, *args], env=env, check=True)

bucket_uri = "s3://" + record["bucket"]
expected = b"Hello DataVault\n"
with tempfile.TemporaryDirectory() as directory:
    path = Path(directory) / "hello.txt"
    if phase == "write":
        path.write_bytes(expected)
        aws("s3", "mb", bucket_uri)
        aws("s3", "cp", str(path), bucket_uri + "/hello.txt")
    aws("s3", "ls", bucket_uri + "/")
    path = Path(directory) / "downloaded.txt"
    aws("s3", "cp", bucket_uri + "/hello.txt", str(path))
    if path.read_bytes() != expected:
        raise SystemExit("Downloaded bytes differ")
    print("Verified SHA-256", hashlib.sha256(expected).hexdigest())
if phase == "verify":
    aws("s3", "rm", bucket_uri + "/hello.txt")
    aws("s3", "rb", bucket_uri)
    admin("DELETE", "access-keys/" + record["key"]["id"])
    admin("DELETE", "service-accounts/" + record["user"])
    state.unlink()
