# Contributing

For a change to how agents work, describe the task it makes possible. A useful issue shows where an agent lost the source, misunderstood a decision or could not hand work to the next session. A pull request can demonstrate the fix by giving the next session the task and the library, then checking what it could decide from the evidence. Keep first-use blockers visible and record convenience proposals for later review. [Library method](docs/method.md)

- Register each library in the installation’s JSON registry. Legacy PostgreSQL deployments use the registry table.
- Keep deployment-specific names, hosts, accounts and data in the installation’s configuration and environment. Use fictional records in shared examples.
- One library per information boundary. A tool reaches another library only through what the registry and the git
  server allow; the central library may read the others, never the other way round.
- Implement runtime features in the Rust core and reuse them through the Docker entry point. Keep Python for existing compatibility adapters. Justify and pin new dependencies in the lockfile.
- Write a failing test first, then the fix. Run the tests in [the verification guide](docs/verification.md) before a pull request and put the
  pass/fail counts in it.

## From proposal to release

Open an issue for an installation blocker, a reproducible bug, or a workflow you cannot complete. Small fixes can go straight to a pull request. Use fictional records when showing a failed handoff. For sensitive findings, follow [Security](SECURITY.md).

Anyone can propose or review a change. Repository maintainers decide what to merge and release. If a change affects how existing libraries are read or written, describe what happens to earlier records. Use the [changelog](CHANGELOG.md) to see what is pending for release.

The [community conduct guidelines](CODE_OF_CONDUCT.md) apply in issues and reviews.
