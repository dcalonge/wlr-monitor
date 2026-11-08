#!/bin/bash
set -e
version="${GITHUB_REF_NAME}"

# Nix sha256 is base32 of the raw sha256 digest.
compute_hash() {
    python3 -c "
import hashlib, base64, sys
with open(sys.argv[1], 'rb') as f:
    digest = hashlib.sha256(f.read()).digest()
print(base64.b32encode(digest).decode().lower())
" "$1"
}

hash_x86=$(compute_hash binaries/wlr-monitor-x86_64-unknown-linux-gnu)
hash_arm=$(compute_hash binaries/wlr-monitor-aarch64-unknown-linux-gnu)

python3 - <<'PYEOF'
import re, sys
version = sys.argv[1]
hash_x86 = sys.argv[2]
hash_arm = sys.argv[3]
with open('flake.nix') as f:
    content = f.read()
content = re.sub(r'binVersion = ".*"', f'binVersion = "{version}"', content)
content = content.replace('sha256-REPLACE_X86_64', hash_x86)
content = content.replace('sha256-REPLACE_AARCH64', hash_arm)
with open('flake.nix', 'w') as f:
    f.write(content)
PYEOF

git config user.name "github-actions"
git config user.email "github-actions@github.com"
git remote set-url origin "https://x-access-token:${GITHUB_TOKEN}@github.com/dcalonge/wlr-monitor.git"
git add flake.nix
git commit -m "Update flake.nix bin hashes for ${version} [skip ci]" || true
git push origin master || true
