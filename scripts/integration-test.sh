#!/usr/bin/env sh
set -eu
docker compose run --rm postgres-tests
docker compose run --rm --build integration-tests
