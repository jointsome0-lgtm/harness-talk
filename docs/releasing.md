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

1. Update the version in `Cargo.toml` and refresh `Cargo.lock`; Python package metadata reads that version through maturin. Update any unreleased notices and review the final commit on `main`. For a storage change, verify upgrades from every supported legacy schema, including backup contents, concurrent opens and failure rollback.
2. Run `publish.yml` on that commit with mode `build-only`. Confirm the installed-wheel tests and metadata checks pass and the upload job is skipped.
3. With owner authorization to publish, run the workflow with mode `publish`. Check that the commit has not changed. Merge nothing else until the release is tagged: the workflow builds the current `main`. Inspect this run's build output, distributions, and SHA-256 hashes; its upload job waits for approval of the `pypi` environment.
4. Before approving upload, run the live gate on the wheel and source archive from that exact publish run, installed and extracted into fresh state. For each available installed native client, observe the actual notice; show the full request once; validate identifier, sender, recipient, body and reply relationship before a separate ACK; obtain the correlated reply; read and separately ACK it at the controller; and record empty final inboxes. Record date, source commit, run, artifact hashes, installed versions and permission mode. Distinguish actual model calls from deterministic local providers. Record native exit and owned PID/start/pidfd/group/listener closure separately, including unsupported or unobserved parts. Keep this report outside the repository until publication, so the checked commit and artifacts stay the ones being approved. Keep literal `notification_cleanup` results; `absent`, `unsupported` and `skipped` do not prove removal. Preserve failed or uncertain attempts, inspect their saved state and adjudicate before any separately authorized fresh case. A client lacking installed prerequisites is not checked. Review the complete scope and limitations before approving that run's artifacts.

   For client receivers, extract the source archive from this same workflow run
   and verify that it contains every adapter file from the release commit.
   Set `HTALK_SOURCE` to that archive, and `HTALK_BIN` and each MCP command to
   the wheel virtual environment's `htalk`. Keep that environment first on the
   client `PATH`; do not build or select a checkout executable for this check.
5. Approve the `pypi` environment for that run only after reviewing the report, commit and hashes. The upload job downloads that run's checked artifacts; it does not rebuild them.
6. Verify the [PyPI release](https://pypi.org/project/harness-talk/) and install its version in a fresh virtual environment. Run `htalk --version`, `htalk --help`, and `python -m pip check`. Compare the downloaded files with the workflow's hashes.
7. Tag the published commit with a lightweight tag `vVERSION`, as for earlier releases, and push the tag.
8. Record the live check results in `docs/adapters.md` and `docs/opencode.md` in a separate pull request.
9. Release notes and authorized announcements must say whether a mailbox migration is needed and whether it happens automatically. Include the backup location, which installations need updating, any required command, the post-upgrade checks and recovery limits. For automatic upgrades, say that the first ordinary command performs the migration and that htalk never restores an old backup automatically.
10. Every release's notes and announcements must name the people or agents who helped, with their specific contribution and a verifiable source. Credit code, tests, review and useful feedback accurately. A documentation review is not a live test, and praise alone is not evidence of a tested contribution.

Only the upload job receives `id-token: write`. The official PyPA action verifies metadata and produces attestations. A successful build alone does not verify the trust configuration; a successful authorized upload does.

PyPI files cannot be overwritten. After a partial or uncertain failure, inspect the release before another upload.
