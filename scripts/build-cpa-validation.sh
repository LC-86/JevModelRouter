#!/bin/sh
# Explicit local build from a verified source archive. No download or system installation.
set -eu
go_binary=${1:?Provide the verified Go 1.26.0 executable}
source_archive=${2:?Provide the pinned CPA source tar.gz}
output_binary=${3:?Provide an absolute output path outside the checkout}
case "$output_binary" in /*) ;; *) echo 'Output must be an absolute path' >&2; exit 1;; esac
build_root=$(mktemp -d "${TMPDIR:-/tmp}/jev-r1-cpa-build.XXXXXX")
trap 'rm -rf "$build_root"' EXIT HUP INT TERM
cache_root=${4:-"$build_root"}
case "$cache_root" in /*) ;; *) echo 'Cache root must be absolute' >&2; exit 1;; esac
python3 - "$source_archive" "$build_root" <<'PY'
import hashlib,pathlib,sys,tarfile
p=pathlib.Path(sys.argv[1])
assert hashlib.sha256(p.read_bytes()).hexdigest()=='ac4e6e84142973138367efa2e734f08ed2a1b06c75c979e2a8837a09ffb09661','CPA source archive checksum mismatch'
with tarfile.open(p) as archive: archive.extractall(sys.argv[2],filter='data')
PY
case "$("$go_binary" version)" in 'go version go1.26.0 darwin/arm64') ;; *) echo 'Use verified Go 1.26.0 darwin/arm64' >&2; exit 1;; esac
source_root="$build_root/CLIProxyAPI-e2bff0107bb307337aaa19018ccddd55f64253d5"
cd "$source_root"
env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin GOENV=off GOTOOLCHAIN=local GOCACHE="$cache_root/cpa-build-cache" GOMODCACHE="$cache_root/cpa-module-cache" \
  "$go_binary" build -trimpath -ldflags '-X main.Version=r1-e2bff010 -X main.Commit=e2bff0107bb307337aaa19018ccddd55f64253d5 -X main.BuildDate=2026-10-02' -o "$output_binary" ./cmd/server
python3 - "$output_binary" <<'PY'
import hashlib,pathlib,sys
checksum=hashlib.sha256(pathlib.Path(sys.argv[1]).read_bytes()).hexdigest()
assert checksum=='b92fd27a406361a409c04e2c9e1ac54f864e7ecfd4ac202b48ef164038586809','CPA artifact checksum mismatch'
print(checksum)
PY
cp LICENSE "$output_binary.LICENSE"
