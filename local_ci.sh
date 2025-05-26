#!/bin/bash

set -ex

CI=1 cargo machete
CI=1 cargo clippy -p openvaf --all-targets -- -D warnings
CI=1 cargo nextest run

typos

git diff-files --quiet \
  || (echo "The working directory is dirty commit or stash your changes" \
  && exit 1)
