"""Run with python benchmark.py --objects 30 --mib 4 --workers 4 inside the test image."""
import argparse
import concurrent.futures
import json
import statistics
import time
import uuid

from client import account, admin, s3

parser = argparse.ArgumentParser()
parser.add_argument("--objects", type=int, default=20)
parser.add_argument("--mib", type=int, default=1)
parser.add_argument("--workers", type=int, default=2)
args = parser.parse_args()
if min(args.objects, args.workers, args.mib) < 1:
    parser.error("All dimensions must be positive")
user, key = account(True)
client = s3(key)
bucket = "bench-" + uuid.uuid4().hex
client.create_bucket(Bucket=bucket)
payload = bytes(args.mib * 1024 * 1024)

def transfer(index):
    name = f"bench/{index:09d}"
    begin = time.perf_counter()
    client.put_object(Bucket=bucket, Key=name, Body=payload)
    put = time.perf_counter() - begin
    begin = time.perf_counter()
    body = client.get_object(Bucket=bucket, Key=name)["Body"]
    size = sum(map(len, body.iter_chunks(65536)))
    assert size == len(payload)
    return put, time.perf_counter() - begin

try:
    begin = time.perf_counter()
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.workers) as pool:
        times = list(pool.map(transfer, range(args.objects)))
    elapsed = time.perf_counter() - begin
    begin = time.perf_counter()
    result = list(client.get_paginator("list_objects_v2").paginate(Bucket=bucket, Prefix="bench/"))
    assert sum(page["KeyCount"] for page in result) == args.objects
    print(json.dumps({"objects": args.objects, "MiB_per_object": args.mib, "workers": args.workers,
                      "combined_MiB_per_second": args.mib * args.objects * 2 / elapsed,
                      "median_put_seconds": statistics.median(p[0] for p in times),
                      "median_get_seconds": statistics.median(p[1] for p in times),
                      "list_seconds": time.perf_counter() - begin}, indent=2))
finally:
    for page in client.get_paginator("list_objects_v2").paginate(Bucket=bucket):
        for obj in page.get("Contents", []):
            client.delete_object(Bucket=bucket, Key=obj["Key"])
    client.delete_bucket(Bucket=bucket)
    admin("DELETE", "service-accounts/" + user)
