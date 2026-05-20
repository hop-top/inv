# Releasing

## Version Lifecycle

```
0.1.0-alpha.1 -> .2 -> ... -> 0.1.0-beta.1 -> ... -> 0.1.0-rc.1 -> ... -> 0.1.0
```

| Stage   | Audience     | API              | Breaking changes  |
|---------|--------------|------------------|-------------------|
| alpha   | contributors | unstable         | expected          |
| beta    | testers      | feature-complete | only if critical  |
| rc      | everyone     | frozen           | showstoppers only |
| release | everyone     | stable           | next major only   |

## How releases work

1. Conventional commits on `main` trigger release-please
2. release-please creates/updates a release PR with version
   bumps + changelog
3. Merging the release PR creates GitHub Releases + tags
4. Per-language publish jobs fire automatically

## Promoting a release stage

Interactive:

```bash
make promote
```

Explicit:

```bash
make promote-alpha    # initialize alpha (new version cycle)
make promote-beta     # alpha -> beta (feature-complete)
make promote-rc       # beta -> rc (no known blockers)
make promote-release  # rc -> release (bake period passed)
```

### Transition criteria

| Transition     | Criteria                         |
|----------------|----------------------------------|
| -> alpha       | new version cycle starts         |
| alpha -> beta  | all planned features merged      |
| beta -> rc     | no known bugs blocking release   |
| rc -> release  | 7-day bake, no regressions       |

## Nightly auto-release

A cron workflow runs nightly at 04:00 UTC. If a release-please PR
exists and CI is green, it auto-merges — producing a release without
manual intervention.

To disable: set the `NIGHTLY_RELEASE` repo variable to `false`, or
disable the workflow in GitHub Actions settings.

## Version synchronization

For polyglot projects, major.minor stays synchronized across
all language ports via release-please `linked-versions`. Patch
versions may differ.
