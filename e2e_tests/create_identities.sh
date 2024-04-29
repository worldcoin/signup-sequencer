#!/bin/sh

NUMBER=${1:-1}
SLEEP=${2:-0}

for run in $(seq $NUMBER); do
  echo "running";
  curl -X POST -H "Content-Type: application/json" -d "{\"identityCommitment\":\"0x$(openssl rand -hex 16)\"}" localhost:9080/insertIdentity -vv;
  sleep $SLEEP
done