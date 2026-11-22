"""Advisory hooks only: no Cargo, source edits, staging, or network access.

Stop checks tracked changes against HEAD, including the index, not untracked files.
"""

import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tempfile
import time


DEADLINE = time.monotonic() + 2.0
BUILD_FILES = {
    "Cargo.toml", "Cargo.lock", "build.rs", "rust-toolchain", "rust-toolchain.toml",
    "rustfmt.toml", ".rustfmt.toml", "clippy.toml", ".clippy.toml", ".gitignore",
}
AREAS = {
    "Camera Integrator": ("camera", "environment", "light", "src/sampling/"),
    "Photon Integrator": ("photon", "caustic"),
    "Material / BSDF": ("material", "bsdf"),
    "Optics": ("optics", "refract", "dispersion"),
    "Spectrum": ("spectrum", "spectral", "wavelength", "assets/color/", "src/color.rs"),
    "Medium": ("medium", "volume", "fog"),
    "Geometry / Scene": ("geometry", "prism", "sphere", "src/scene/"),
    "Diagnostics": ("diagnostics", "path_debug"),
    "Configuration / Output": (
        "src/config/", "configs/", "output", "src/film.rs", ".cargo/",
        "cargo.toml", "cargo.lock", "rust-toolchain", ".github/workflows/", ".gitignore", "src/renderer.rs",
    ),
}


def remaining():
    seconds = DEADLINE - time.monotonic()
    if seconds <= 0:
        raise TimeoutError("hook time budget exhausted")
    return seconds


def git(cwd, *args, check=True):
    return subprocess.run(
        ["git", "--no-optional-locks", "-C", str(cwd), "-c", "core.quotePath=false", *args],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        timeout=min(1.0, remaining()), check=check,
    )


def changed_paths(root):
    records = iter(git(root, "status", "--porcelain=v1", "-z", "--untracked-files=all").stdout.split(b"\0"))
    paths = {}
    for record in records:
        if not record:
            continue
        status = record[:2]
        paths[os.fsdecode(record[3:])] = status
        if b"R" in status or b"C" in status:
            paths[os.fsdecode(next(records))] = status
    return paths


def relevant(path):
    return (
        path.endswith(".rs") or path in BUILD_FILES
        or path.startswith(".cargo/")
        or (path.startswith("configs/") and path.endswith((".json", ".toml")))
        or (path.startswith(".github/workflows/") and path.endswith((".yml", ".yaml")))
        or (path.startswith("assets/color/") and not path.endswith(".md"))
    )


def fingerprint(root, paths):
    digest = hashlib.sha256()
    digest.update(git(root, "rev-parse", "--verify", "HEAD", check=False).stdout)
    # Index blob IDs catch staged changes even when the working file is unchanged.
    digest.update(git(root, "ls-files", "--stage", "-z", "--", *paths).stdout)
    for name, status in sorted(paths.items()):
        remaining()
        digest.update(os.fsencode(name) + b"\0" + status)
        path = root / name
        try:
            mode = path.lstat().st_mode
        except FileNotFoundError:
            digest.update(b"deleted")
            continue
        digest.update(str(mode).encode())
        if stat.S_ISLNK(mode):
            digest.update(os.fsencode(os.readlink(path)))
        elif stat.S_ISREG(mode):
            with path.open("rb") as source:
                for chunk in iter(lambda: source.read(65536), b""):
                    remaining()
                    digest.update(chunk)
    return digest.hexdigest()


def session_cache(event):
    session = event.get("session_id")
    if not isinstance(session, str) or not session:
        return None
    key = hashlib.sha256(session.encode()).hexdigest()
    return Path(tempfile.gettempdir()) / f"codex-quality-{os.getuid()}-{key}.json"


def already_reported(event, root, value):
    cache = session_cache(event)
    if cache is None or root == cache.parent.resolve() or root in cache.parent.resolve().parents:
        return False  # Never put session state inside the repository.
    try:
        descriptor = os.open(cache, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
        with os.fdopen(descriptor, "r+") as state:
            # Parallel tools must not wait on, or emit the same feedback as, each other.
            fcntl.flock(state, fcntl.LOCK_EX | fcntl.LOCK_NB)
            try:
                values = json.loads(state.read(65536))
            except ValueError:
                values = {}
            if not isinstance(values, dict):
                values = {}
            root_key = hashlib.sha256(os.fsencode(root)).hexdigest()
            if values.get(root_key) == value:
                return True
            values[root_key] = value
            state.seek(0)
            state.truncate()
            json.dump(values, state)
        return False
    except BlockingIOError:
        return True
    except (OSError, ValueError):
        return False  # Deduplication failure must not suppress verification guidance.


def post_tool_use(event, root):
    paths = {name: status for name, status in changed_paths(root).items() if relevant(name)}
    if not paths or already_reported(event, root, fingerprint(root, paths)):
        return {}
    areas = sorted(name for name, patterns in AREAS.items() if any(
        pattern in path.lower() for path in paths for pattern in patterns
    ))
    examples = json.dumps([path[:160] for path in sorted(paths)[:4]], ensure_ascii=False)
    guidance = (
        "当前工作区存在需验证的变更，可能包含会话前已有修改，不归因于本次工具调用。"
        f"区域：{'、'.join(areas) or '共享 Rust / 测试 / 示例 / 构建'}；路径示例：{examples}。"
        "请按 .agents/testing.md 选择相关数学/行为测试及受影响的 Pipeline 集成测试；"
        "后续相关 diff 变化后应重新核对已有验证并更新验收证据。"
        "此 Hook 未运行测试，也不表示测试通过；强制验收由 CI 负责。"
    )
    return {"hookSpecificOutput": {"hookEventName": "PostToolUse", "additionalContext": guidance}}


def stop(root):
    result = git(root, "diff", "--check", "HEAD", "--", check=False)
    if result.returncode == 0:
        return {}
    changed = {os.fsdecode(path) for path in
               git(root, "diff", "--name-only", "-z", "HEAD", "--").stdout.split(b"\0") if path}
    # Diff diagnostics also contain source lines; report only actual changed paths.
    paths = sorted({match.group(1) for line in os.fsdecode(result.stdout).splitlines()
                    if (match := re.match(r"^(.+):[0-9]+:", line)) and match.group(1) in changed})
    if not paths:
        raise RuntimeError("diff check unavailable")
    shown = json.dumps([path[:160] for path in paths[:6]], ensure_ascii=False)
    extra = f"（共 {len(paths)} 个路径）" if len(paths) > 6 else ""
    return {"systemMessage": (
        f"git diff --check HEAD -- 发现空白问题：{shown}{extra}。"
        "仅检查已跟踪差异（含暂存），未检查未跟踪文件；此提示不阻断开发。"
    )}


def handle(event):
    if not isinstance(event, dict):
        return {}
    name = event.get("hook_event_name")
    if name not in ("PostToolUse", "Stop"):
        return {}
    try:
        cwd = event.get("cwd")
        if not isinstance(cwd, str) or not cwd:
            return {}
        if name == "PostToolUse" and event.get("tool_name") not in ("apply_patch", "Bash"):
            return {}
        root = Path(os.fsdecode(git(cwd, "rev-parse", "--show-toplevel").stdout).rstrip("\n")).resolve()
        return post_tool_use(event, root) if name == "PostToolUse" else stop(root)
    except Exception:
        if name == "Stop":
            return {"systemMessage": (
                "质量 Hook 未能完成 git diff --check HEAD --，请手动核对；"
                "未跟踪文件不在检查范围内。此提示不阻断开发。"
            )}
        return {}
    finally:
        if name == "Stop":
            try:
                cache = session_cache(event)
                if cache is not None:
                    cache.unlink(missing_ok=True)
            except OSError:
                pass


def main():
    try:
        output = handle(json.load(sys.stdin))
    except Exception:
        output = {}
    print(json.dumps(output))


if __name__ == "__main__":
    main()
