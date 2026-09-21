# Releasing harness-talk

The `publish.yml` workflow runs manually. Its default `build-only` mode builds a source distribution and Linux x86-64/ARM64 binary wheels, checks their metadata, and tests each installed wheel outside the checkout. Each wheel targets glibc 2.28 or newer. The x86-64 build also rebuilds from the source distribution. Pushes and tags do not trigger publishing. The upload job runs only when a maintainer selects `publish` on `main`.

## Trust configuration

The GitHub environment `pypi` accepts only the `main` branch and requires the repository owner's review before upload. PyPI trusts these exact values:

| Field | Value |
| --- | --- |
| PyPI project | `harness-talk` |
| GitHub owner | `jointsome0-lgtm` |
| Repository | `harness-talk` |
| Workflow filename | `publish.yml` |
| Environment | `pypi` |

For a new project, configure a pending publisher in [PyPI account publishing](https://pypi.org/manage/account/publishing/). It creates the project on first use; it does not reserve the name. See [PyPI's pending-publisher guide](https://docs.pypi.org/trusted-publishers/creating-a-project-through-oidc/). No API token or password is stored in GitHub secrets.

## Release procedure

1. Update the version in `Cargo.toml` and refresh `Cargo.lock`; Python package metadata reads that version through maturin. Review the final commit on `main`.
2. Run `publish.yml` on that commit with mode `build-only`. Confirm the installed-wheel tests and metadata checks pass and the upload job is skipped.
3. With owner authorization to publish, run the workflow with mode `publish`. Check that the commit has not changed. Merge nothing else until the release is tagged: the workflow builds the current `main`. Inspect this run's build output, distributions, and SHA-256 hashes; its upload job waits for approval of the `pypi` environment.
4. Before that approval, run the live check on this run's wheel, downloaded and installed in a fresh virtual environment. With each supported client on its installed version, exchange one notified request and its reply in separate terminal sessions. For each client, record whether the notification arrived and the reply was correlated with the request, then record `ack` and `notification_cleanup` separately. Record the date, commit SHA, wheel hash, htalk and client versions, and each client's permission mode. A client that is not installed is recorded as not checked. Keep this report outside the repository until publication, so the checked commit and artifacts stay the ones being approved.
5. Approve the `pypi` environment for that run only after reviewing the report, commit and hashes. The upload job downloads that run's checked artifacts; it does not rebuild them.
6. Verify the [PyPI release](https://pypi.org/project/harness-talk/) and install its version in a fresh virtual environment. Run `htalk --version`, `htalk --help`, and `python -m pip check`. Compare the downloaded files with the workflow's hashes.
7. Tag the published commit with a lightweight tag `vVERSION`, as for earlier releases, and push the tag.
8. Record the live check results in `docs/adapters.md` and `docs/opencode.md` in a separate pull request.

Only the upload job receives `id-token: write`. The official PyPA action verifies metadata and produces attestations. A successful build alone does not verify the trust configuration; a successful authorized upload does.

PyPI files cannot be overwritten. After a partial or uncertain failure, inspect the release before another upload.
