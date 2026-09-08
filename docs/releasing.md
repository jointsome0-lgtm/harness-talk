# Releasing harness-talk

The `publish.yml` workflow runs manually. Its default `build-only` mode builds a source distribution and a wheel, checks their metadata, and tests the installed wheel outside the checkout. Pushes and tags do not trigger publishing. The upload job runs only when a maintainer selects `publish` on `main`.

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

1. Update `pyproject.toml` and `src/harness_talk/__init__.py` to the same version. Review the final commit on `main`.
2. Run `publish.yml` on that commit with mode `build-only`. Confirm the installed-wheel tests and metadata checks pass and the upload job is skipped.
3. With owner authorization to publish, run the workflow with mode `publish`. Check that the commit has not changed. Inspect this run's build output, distributions, and SHA-256 hashes before approving its `pypi` environment. The upload job downloads those exact artifacts by ID; it does not rebuild them.
4. Verify the [PyPI release](https://pypi.org/project/harness-talk/) and install its version in a fresh virtual environment. Run `htalk --version`, `htalk --help`, and `python -m pip check`. Compare the downloaded files with the workflow's hashes.

Only the upload job receives `id-token: write`. The official PyPA action verifies metadata and produces attestations. A successful build alone does not verify the trust configuration; a successful authorized upload does.

PyPI files cannot be overwritten. After a partial or uncertain failure, inspect the release before another upload.
