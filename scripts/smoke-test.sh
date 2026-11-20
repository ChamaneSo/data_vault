#!/usr/bin/env sh
set -eu
docker compose run --rm --build integration-tests pytest -q -p no:cacheprovider -k aws_cli
