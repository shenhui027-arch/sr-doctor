#!/bin/bash
# Closed-loop business client for the multi-BE product-footprint check (v0.6-10).
# Logic only, not a performance test: a few light queries on e2e_fixture.
# Runs inside the owned footprint container. Usage:
#   workload.sh CLIENT_ID QUERY_DIR OUT_DIR FE_HOST FE_PORT PROFILE(0|1)
# The workload_user password arrives only through the inherited MYSQL_PWD.
# Each statement appends: phase,client,query,latency_us,exit_code,start_unix_ns
set -u
id="$1"; qdir="$2"; out="$3"; host="$4"; port="$5"; profile="$6"
mapfile -t files < <(ls "$qdir"/*.sql | sort)
count=${#files[@]}
next=$id
while [ ! -f "$out/stop" ]; do
  phase=$(cat "$out/phase" 2>/dev/null || echo warmup)
  file=${files[$((next % count))]}
  next=$((next + 1))
  start=$(date +%s%N)
  {
    if [ "$profile" = "1" ]; then echo "SET enable_profile=true;"; fi
    cat "$file"
  } | mysql --batch --skip-column-names --host="$host" --port="$port" \
      --user=workload_user --database=e2e_fixture > /dev/null 2>> "$out/client-$id.err"
  code=$?
  end=$(date +%s%N)
  echo "$phase,$id,$(basename "$file" .sql),$(( (end - start) / 1000 )),$code,$start" >> "$out/client-$id.csv"
done
