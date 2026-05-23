# .githooks

Repo-local git hooks. Opt-in per clone — git does not auto-discover this
directory.

## What's here

| Hook       | What it does                                                |
|------------|-------------------------------------------------------------|
| `pre-push` | Runs `cargo fmt --all -- --check`. Blocks push on drift.    |

## Enable (one-time per clone)

```sh
git config core.hooksPath .githooks
```

That points git at this directory for hook resolution. Unsetting:

```sh
git config --unset core.hooksPath
```

## Bypass

Two ways, both intentional escape hatches:

```sh
INV_SKIP_FMT_HOOK=1 git push   # documented, hook-specific
git push --no-verify           # git-native, skips all hooks
```

Use sparingly — CI runs `cargo fmt --check` anyway and will reject the push
on the server side.

## Why pre-push (not pre-commit)

- `pre-commit` fires on every commit — too noisy when iterating.
- `pre-push` catches drift right before it leaves the local repo, mirroring
  where CI would catch it next.
- `pre-push` sees the full diff being pushed, not a single commit.

## Adding more hooks

Drop a new executable script in this directory named after the git hook
stage (`pre-commit`, `commit-msg`, `pre-push`, etc.). `chmod +x` it. No
config change needed once `core.hooksPath` is set.

## Why not clippy here?

Clippy compiles. Workspace clippy on a cold cache is 30s+. Hooks should be
fast. If we want a clippy gate, it lives as a separate opt-in hook or in
CI — not bundled with fmt.
