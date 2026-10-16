"""Point the release configuration and the README links at a GitHub repository.

    python tools/set_repo.py OWNER/REPO [--tag v1.0.0]

Updates release.json (repo, tag, pages_url) and replaces the OWNER/REPO
placeholders (or the previously configured repository) in README.md,
README.ja.md and the in-game data pack link (regenerate it with
tools/gen_datapack.py afterwards).
"""
import json
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def main():
    if len(sys.argv) < 2 or "/" not in sys.argv[1]:
        raise SystemExit(__doc__)
    repo = sys.argv[1].strip("/")
    owner, name = repo.split("/", 1)
    cfg_path = os.path.join(ROOT, "release.json")
    cfg = json.load(open(cfg_path, encoding="utf-8"))
    old_repo = cfg["repo"]
    old_owner, old_name = old_repo.split("/", 1)
    old_tag = cfg["tag"]
    if "--tag" in sys.argv:
        cfg["tag"] = sys.argv[sys.argv.index("--tag") + 1]
    cfg["repo"] = repo
    cfg["pages_url"] = f"https://{owner.lower()}.github.io/{name}/"
    with open(cfg_path, "w", encoding="utf-8", newline="\n") as f:
        json.dump(cfg, f, ensure_ascii=False, indent=2)
        f.write("\n")
    for fn in ("README.md", "README.ja.md"):
        p = os.path.join(ROOT, fn)
        s = open(p, encoding="utf-8").read()
        s = s.replace(f"https://{old_owner}.github.io/{old_name}/", cfg["pages_url"])
        s = s.replace(f"https://{old_owner.lower()}.github.io/{old_name}/", cfg["pages_url"])
        s = s.replace(f"github.com/{old_repo}/", f"github.com/{repo}/")
        s = s.replace(f"/releases/download/{old_tag}/", f"/releases/download/{cfg['tag']}/")
        with open(p, "w", encoding="utf-8", newline="\n") as f:
            f.write(s)
    print(f"repository {repo}, tag {cfg['tag']}, pages {cfg['pages_url']}")
    print("next: python tools/gen_datapack.py  (puts the web link into the in-game menu)")


if __name__ == "__main__":
    main()
