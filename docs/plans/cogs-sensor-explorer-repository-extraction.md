# Cogs and Sensor Explorer repository extraction

Imported 2026-10-27 from the WeftOS repository, `docs/plans/cogs-sensor-explorer-repository-extraction.md` (dated 2026-10-06, committed on WeftOS `feat/post-v0.8.3`). The private repository name and the coordinator handoff section were left out. The repository has since moved from `weave-logic-ai/weftos-cogs` to `WeftOS/weftos-cogs`; the text below keeps the names it was written with. Open work from this plan and the Phase A records is staged as tasks in [cogs-extraction-tasks.manifest.json](cogs-extraction-tasks.manifest.json). The Phase A ledgers and proof report stay in WeftOS: each says not to import it here, because they name local checkouts and the private repository.

Status: proposed for approval. No code or repository history has been moved.

Date: 2026-10-06

Release constraint: WeftOS 0.8.3 is being released. Extraction starts only after its release commit and tag are final.

## Purpose

Move the Cogs product, hardware catalog, Sensor Explorer, appliance host, browser console, and sensor-guide UI out of the WeftOS operating-system repository. Keep WeftOS responsible for the trusted operating-system primitives: identity, governance, placement, mesh, chains, supervision, and a stable Cog protocol.

The result should be one public Cogs product repository with one build and release graph, while the private Cognitum client/upstream workspace remains separate. This avoids three mutable catalogs and prevents client material from entering a public repository.

## Recommendation

Use the existing public repository `weave-logic-ai/weftos-cogs` as the destination. Expand it into a domain workspace instead of creating another repository.

Keep the private Cognitum repository separate. It owns Cognitum-specific upstream work, device operations, private/client context, and contribution branches. Move or copy only reviewed, redistributable Cog source into `weftos-cogs` through normal pull requests.

Import the local Sensor Explorer project into `weftos-cogs` under `apps/sensor-explorer`. It is the public catalog website, REST API, MCP server, D1 schema, contribution flow, guide service, and signed Cog artifact endpoint. Its current manual `catalog.seed.json` copy becomes a generated artifact from the repository's canonical catalog.

Do not move the WeftOS dashboard. It remains a separate consumer that links to the Cogs console and Sensor Explorer through versioned URLs and APIs.

The Weave Manager extraction seam is already implemented. Commit `d2c0d4270` added the `weave-manager` product entry point, local address book, mesh key/session enrollment, and node tree. It is reachable from `origin/target-0.8.3` and the 0.8.3 release-candidate tags. The temporary `weave-manager` branch and worktree were removed only after that commit was merged. Extraction must reuse this prepared cherry-pick lane rather than fork the manager again.

## Repository boundary

### Move to `weftos-cogs`

| Current source | Destination | Notes |
|---|---|---|
| Existing `weftos-cogs` Cog sources and tooling | `src/cogs/`, `crates/cog-sensor-sources/`, `scripts/` | Retain the current public repository history and layout. |
| `crates/weftos-cog-repo` | `crates/cog-repo` | Signing, verification, registry, and publishing tools. |
| `crates/weftos-cog-market` | `crates/cog-market` | Canonical software and hardware catalog. |
| `crates/weftos-cog-sources` | `crates/cog-sources` | Multi-source resolution, verified fetch, and project source policy. |
| `crates/weftos-cog-host` | `crates/cog-host` | Appliance lifecycle, hardware discovery, installation, and licence enforcement. |
| `crates/weftos-cog-manager` | `crates/cog-manager` | Import the existing shared crate once and retain `weave-manager` as its product entry point. Use the prepared cherry-pick series ending at `d2c0d4270`; do not create another manager fork. |
| `crates/weftos-cog-companion` | `crates/cog-companion` | Shared native/WASM sensor application shell. |
| `crates/weftos-sensor-guide` | `crates/sensor-guide` | Guide format, validation, diagrams, and renderer. |
| `crates/weftos-guide-view` | `crates/guide-view` | Standalone browser guide viewer. |
| `crates/weftos-{ecg,tof,sound}-scope` | `apps/{ecg,tof,sound}-scope` | Second import wave after the shared companion API is stable. |
| Sensor Explorer local repository | `apps/sensor-explorer` | Worker, D1 migrations, MCP, public API, website, guides, and publishing endpoints. |
| Cogs build and conformance portions of `scripts/build.sh` and `scripts/cogs/` | `scripts/` and `xtask/` | Make the new repository independently buildable. |
| Generic Cogs and hardware docs | `docs/` | Sanitize before import; preserve ADR lineage. |

The repository may remain a monorepo internally. The boundary being removed is the Cogs product's coupling to the WeftOS OS monorepo, not useful coupling among the Cogs product crates.

### Keep in WeftOS

- Workload-kind interfaces, governed placement, package admission, artifact transport, runtime adapters, mesh, chains, identity, and supervision.
- The generic daemon RPC transport and a small, versioned Cog protocol/client crate.
- Thin `weaver cog ...` command adapters while compatibility is needed.
- ADR-100's WeftOS hosting contract. Cogs product decisions remain in the COG decision series.
- Generic release/update machinery and OS service management.
- Dashboard integration contracts, without the dashboard source itself.

### Keep private or external

- Cognitum client operations, device backups, account details, commercial research, private firmware facts, and confidential notes.
- Cognitum upstream contribution worktrees until their branches are merged or deliberately transferred.
- Dashboard source and deployment configuration.
- Machine-local tokens, `.dev.vars`, signing seeds, release keys, tailnet addresses, and Terraform state.

## Target layout

```text
weftos-cogs/
  Cargo.toml
  package.json
  src/cogs/                    # signed sensor and hardware cogs
  crates/
    cog-protocol/              # stable wire types; no kernel dependency
    cog-repo/
    cog-market/
    cog-sources/
    cog-host/
    cog-manager/
    cog-companion/
    sensor-guide/
    guide-view/
  apps/
    sensor-explorer/           # Cloudflare Worker + D1 + MCP + public site
    ecg-scope/
    tof-scope/
    sound-scope/
  catalog/
    sources/                   # reviewed canonical inputs
    generated/                 # reproducible outputs, checked for drift
  docs/
    decisions/                 # COG-NNN decisions
    guides/
    hardware/
    operations/
  scripts/
  xtask/
```

Generated WASM bindings under `www/pkg`, `node_modules`, local databases, `.dev.vars`, target directories, signing material, and generated SQL are not history imports. CI regenerates deployable assets.

## Existing Weave Manager fork

`weave-manager` is a second binary entry point over the existing `weftos-cog-manager` library. It is not a second copied UI implementation. That is the correct extraction shape: one Manager library, the `weave-manager` product name, and native plus browser builds from the same source.

The migration unit is broader than one file. `d2c0d4270` also added the corresponding Cog Host mesh-enrollment and session-key support. Cherry-picking only `src/bin/weave-manager.rs` would compile an alias but omit the behavior that made the fork portable. The extraction lane must:

1. establish the manager, market, repository, guide, and host prerequisites in the destination workspace;
2. replay the reviewed Manager/Host commit sequence, with `d2c0d4270` as the prepared fork milestone;
3. retain the `weave-manager` binary and retire the legacy `weft-cog-manager` binary name only through a separately reviewed compatibility decision;
4. run native, WASM, enrollment, address-book, project-context, and node-tree tests before accepting the cherry-pick;
5. record any conflict resolution as destination-only adaptation, without changing product behavior during extraction.

After the 0.8.3 final tag exists, the inventory lane must generate and review the exact ordered prerequisite list. The current path history begins with the original Manager implementation at `34a90ccc9` and contains many later Host, catalog, guide, fleet, and security changes; treating `d2c0d4270` as a standalone patch without those prerequisites would be unsafe.

## Documentation and decision migration

Documentation moves according to authority, not merely because it mentions a Cog. Accepted WeftOS ADRs remain in WeftOS as historical records. The Cogs repository receives the product documentation and new COG successor decisions, with provenance back to the WeftOS 0.8.3 tag.

### Remain authoritative in WeftOS

| Document | Reason |
|---|---|
| ADR-099, governed workload placement | Defines the generic OS placement plane used by Cogs and other workload kinds. |
| ADR-100, the Cog workload kind | Defines how WeftOS hosts and governs a Cog. Its own header already says it stays in WeftOS. |
| ADR-103, topology and roles | Defines machine, user, project, and leaf responsibilities. |
| Core portions of ADR-105 | Project/user configuration, trust enforcement, and daemon command/RPC boundary remain OS contracts. |
| Core portions of ADR-106 | Mesh identity, governance, licence facts, chain events, and run-gate protocol remain OS contracts. |
| `docs/research/mesh-placement/` | Historical design and evidence for the generic placement implementation. Cogs may link to tagged files or import narrowly selected product evidence. |
| WeftOS release notes and historical migration plans | They describe what shipped in WeftOS and remain true after extraction. |

The Cogs repository gets a concise `docs/platform-contracts/` index linking to immutable tagged versions of these documents and summarizing only the API obligations it must implement. It does not carry a second editable copy of the WeftOS decisions.

### Gain canonical successors in `weftos-cogs`

| WeftOS source | Cogs destination | Treatment |
|---|---|---|
| ADR-104, sensor guides | New public COG decision plus an imported historical copy or source link | Cogs owns guide packaging, schema, renderer, companion integration, and per-Cog content. The original WeftOS ADR remains and links to its successor. |
| ADR-107, hardware-to-software catalog link | New public COG decision plus an imported historical copy or source link | Cogs owns catalog joins, Sensor Explorer, Manager panels, firmware/docs metadata, and UI behavior. |
| Product portions of ADR-105 | Public COG source/repository decision | Cogs owns registry formats, source resolution, publishing, catalog merging, and private-source tooling. The WeftOS ADR continues to own project configuration and trust enforcement. |
| Product portions of ADR-106 | Public COG licence/host decision | Cogs owns Cog Host enforcement and operator/product behavior. WeftOS continues to own the licence protocol and governance facts. |
| `docs/cogs/` including images and evidence | `docs/operations/`, `docs/security/`, `docs/testing/`, and `docs/guides/` | Move canonical operational documentation and repair all relative links and code paths. |
| `docs/guides/fleet-manager.md` and `docs/research/fleet-manager/network-tab-brief.md` | `docs/manager/` | Manager/Fleet product documentation moves; WeftOS keeps only mesh and gateway contracts. |
| `docs/architecture/sensor-node.md` | `docs/architecture/sensor-node.md` | Move the product-side sensor/Cog architecture; retain a short WeftOS placement-boundary link if needed. |
| Hardware catalog documentation and data | `docs/hardware/` and `catalog/sources/` | Move material that feeds Hardware Dex, Sensor Explorer, guides, parser behavior, grading, and purchasing/datasheet metadata. |

### Hardware and research split

The current `docs/hardware/` tree mixes Cogs product data with OS node-placement research. The documentation lane classifies every file before import:

- Cogs owns sensor/module/chip catalogs, manufacturer sources, module types, radar protocols, guide research, grades, Hardware Dex facts, and product-facing vendor notes.
- WeftOS retains generic compute-node capability, accelerator, placement, and machine-runtime research when it applies to more than Cogs.
- A document serving both audiences is split into two maintained documents with explicit dependency links. It is not copied into two independently editable versions.
- Related SDR, RF, ESPARGOS, sensor-head, and hardware research moves only when the Cogs product consumes it. Unrelated world-model or OS sensor-pipeline research remains in WeftOS.

### Existing COG and per-Cog decisions

- Preserve the existing private COG decision series in the private Cognitum repository; do not bulk-copy private operational content into the public repository.
- Review public-safe COG-008 through COG-012 material and publish successors or sanitized copies where the public product requires them.
- Preserve the public per-Cog ADRs already in `weftos-cogs` and reconcile them with active sensor branches.
- Resolve duplicate decision identifiers before the destination documentation gate. The current public repository has two files numbered ADR-166, so one needs a reviewed renumbering with redirects/references updated.
- Every imported decision records source repository, source path, source commit/tag, import date, canonical owner, and successor/predecessor links.

### Documentation acceptance gate

Before Phase A proof can pass:

1. A document ledger classifies every Cog-related ADR, `docs/cogs` file, hardware file, manager/fleet guide, architecture note, and selected research file as retain, move, split, archive, or link.
2. The destination decision index has unique identifiers and no missing dependencies.
3. All relative links, images, code paths, command examples, and repository names resolve in their new locations.
4. Public-repository leak checks reject client paths, private hostnames, credentials, commercial notes, device backups, local-only research, and Terraform state.
5. Documentation builds and link checks run in CI.
6. The WeftOS ADR index marks ownership successors without deleting accepted history.
7. Operator, contributor, security, release, catalog, Sensor Explorer, Manager, guide-authoring, conformance, and rollback documentation are sufficient to operate the independent repository without reading an untagged WeftOS checkout.

## Architectural prerequisite: dependency inversion

The extraction should not replace local path dependencies with Git paths back into WeftOS. That would preserve the coupling and make releases non-reproducible.

The largest blocker is `cog-host` linking the full `clawft-kernel` for licence and gate behavior. Replace that link with one of these stable boundaries:

1. Preferred: `weftos-cog-protocol` contains versioned request/response and event types; Cog Host calls the local user/project daemon over its Unix socket.
2. Transitional: a small published `weftos-kernel-client` crate contains only the transport and protocol, with no kernel implementation.

`cog-repo` currently uses `clawft-types` for runtime paths. Give it a Cogs-owned path resolver or pass the revocation path explicitly. `clawft-weave` currently links `cog-sources` and `cog-repo`; change it to the protocol/client boundary or invoke the independently versioned tooling. `weft-licence` features that link Cogs crates need the same treatment.

Golden tests must pin:

- signed registry and manifest bytes;
- catalog JSON schema and stable item identifiers;
- Cog Host HTTP and local RPC shapes;
- licence decisions and refusal codes;
- guide bundles;
- manager and Sensor Explorer catalog compatibility.

WeftOS and `weftos-cogs` must test the same golden vectors in their respective repositories.

## Canonical data ownership

The Cogs repository becomes the source of truth for Cog manifests, guide bundles, hardware entries, grading rules, and the signed catalog. Sensor Explorer consumes a versioned catalog artifact produced by CI. The browser manager embeds or downloads the same artifact.

Sensor Explorer must stop using a hand-copied snapshot from the WeftOS checkout. Its D1 seed is generated from a tagged catalog release. The database records the catalog version and source digest. Contributions enter a review queue and become catalog changes through pull requests; the production database is not the canonical catalog.

Private sources can publish signed overlays. They do not edit the public canonical catalog directly and they never contribute private metadata to a public build.

## Migration inventory

The migration has three source histories and two classes of active development that must be accounted for before any cleanup.

### WeftOS release source

- Freeze the exact 0.8.3 release commit and tag.
- Record every selected path, its last commit, and its dirty/untracked state.
- Import most selected path history from a clone with `git filter-repo`; never rewrite the WeftOS repository.
- For Weave Manager and its matching Cog Host support, use the already prepared, reviewed cherry-pick lineage ending at `d2c0d4270`. Generate the complete prerequisite list from the final 0.8.3 tag instead of assuming that one commit is standalone.
- Keep compatibility crates or command shims for one release after cutover.

### Existing public `weftos-cogs`

- Preserve its current history as the destination base.
- Reconcile its active build branch and its sensor feature worktrees before changing workspace structure.
- Import WeftOS and Sensor Explorer histories as unrelated, filtered branches into an integration branch. Do not force-push or rewrite the public repository.
- Merge each existing feature branch through its own reviewed pull request after the new workspace root lands.

### Sensor Explorer

- First checkpoint all intended work on its current feature branch. Exclude `.dev.vars`, `node_modules`, generated SQL, local D1 files, and deployment secrets.
- Classify current seed JSON, guides, firmware data, pool data, and Cog artifacts as either canonical source, generated output, or private deployment state.
- Filter its history into `apps/sensor-explorer` and merge it into the Cogs integration branch.
- Move Cloudflare configuration and deployment to the destination repository only after preview deployment and data migration tests pass.

### Private Cognitum workspace and client worktrees

- Keep the private repository separate and update it before making branch decisions; its main checkout and several worktrees contain local changes or are behind their remote.
- Produce a branch ledger for every private worktree: branch, remote status, purpose, owning decision, dirty files, upstream PR, and destination.
- Classify each branch as: remain private; upstream to Cognitum; port as a clean public Cog; already superseded; archive after a signed snapshot.
- Never merge the confidential repository, device backups, or client workspace into `weftos-cogs`.
- Public Cog ports must be clean-room pull requests containing only redistributable code, guides, tests, and public measurements.

### Development projects and consumers

Audit and update at least:

- the existing public Cogs repository and all of its active sensor worktrees;
- Sensor Explorer;
- the private Cognitum Cogs repository and its active contribution worktrees;
- WeftOS dashboard links and console launch parameters;
- deployment/configuration repositories that install Cog Host or publish its environment file;
- any project that imports the catalog snapshot, guide JSON, COG-008 registry, or Cog Host API;
- release automation that builds x86_64, armv7, aarch64, native manager, manager WASM, and guide WASM artifacts.

The migration ledger belongs in the private handoff/operations record. The public plan must not contain client paths, credentials, private hostnames, or deployment state.

## Two-stage execution contract

The work has two top-level phases with a mandatory stop between them. Phase A migrates and proves the independent Cogs repository while leaving the WeftOS copies intact. Phase B removes the duplicate implementation from WeftOS only after the proof gate passes and the user explicitly approves removal. Completion of Phase A does not authorize Phase B.

## Phase A: migrate, integrate, and prove

No source path is deleted from WeftOS during Phase A. Compatibility code may be added, but the existing implementation remains available as the rollback source.

### A0: freeze and ledger

1. Finish, tag, and verify WeftOS 0.8.3.
2. Stop structural changes to the selected Cogs paths during extraction, except release blockers.
3. Create path, branch, consumer, secret, generated-file, and deployment ledgers.
4. Create the documentation and ADR ledger defined above, including canonical owner and successor decisions.
5. Run public-repository leak checks against every proposed import.
6. Record baseline tests and artifact hashes in all three source repositories.

Exit: every source path and active branch has an owner and destination; no uncommitted work is at risk.

### A1: protocol boundary

1. Define `weftos-cog-protocol` and version its wire contracts.
2. Add the client/server implementation in parallel without removing old in-process calls.
3. Move licence and gate requests from Cog Host's kernel link to daemon RPC.
4. Remove `clawft-kernel` and `clawft-types` implementation dependencies from the standalone copies.
5. Add cross-repository golden compatibility tests.

Exit: all selected crates compile in a standalone workspace using released or local protocol crates, with no path back to the WeftOS checkout.

### A2: history-preserving repository import

1. Branch from the current `weftos-cogs` repository; do not use its default branch directly.
2. Import filtered WeftOS history under the target paths, except where a reviewed cherry-pick lane already exists.
3. Replay the prepared Weave Manager and matching Cog Host cherry-pick series, ending at the 0.8.3 version of `d2c0d4270`.
4. Import filtered Sensor Explorer history under `apps/sensor-explorer`.
5. Import the product documentation, create COG successor decisions, and add tagged WeftOS platform-contract links according to the documentation ledger.
6. Establish workspace metadata, license files, security policy, contribution guide, CODEOWNERS, and dependency policy.
7. Reconcile active public sensor branches one by one.

Exit: the new workspace builds locally, preserves source history, and contains no generated binaries or secrets. The original WeftOS paths still exist.

### A3: canonical catalog and web consolidation

1. Select one canonical catalog source format and generate all JSON, guide bundles, D1 seeds, and embedded assets from it.
2. Make Sensor Explorer publish versioned catalog artifacts and signed Cog registry artifacts.
3. Make Cog Manager and Guide View consume those artifacts without a WeftOS checkout.
4. Add preview deployments for Sensor Explorer and browser WASM apps.
5. Update dashboard links only after preview URLs and token/origin behavior pass end-to-end tests.

Exit: website, MCP, REST API, manager, and guide viewer show the same catalog version and digest.

### A4: release and deployment ownership

1. Add independent semantic versions and changelog for Cogs product releases.
2. Build signed source archives, SBOMs, provenance, registries, native binaries, cross-compiled Cog Host binaries, and WASM bundles in Cogs CI.
3. Keep signing in protected environments; use distinct Cog artifact and WeftOS release keys.
4. Update appliance and server deployment repositories to consume a tagged Cogs release.
5. Run canary upgrades on development hardware before changing production services.
6. Update WeftOS to consume a tagged Cog protocol/client release while retaining its existing Cogs source and compatibility path.

Exit: the independent repository owns new Cogs releases and deployments, while the WeftOS copy remains available for comparison and rollback.

### A5: dual-run proof and soak

1. Run the WeftOS full gate, Cogs full gate, dashboard contract tests, and appliance end-to-end tests against the same release candidate.
2. Prove native Manager, Manager WASM, Guide View, Sensor Explorer, MCP, Cog Host on x86_64, Cog Host on ARM, signed installation, licence enforcement, revocation, and rollback.
3. Operate consumers from tagged Cogs releases while comparing protocol results and catalog digests against the retained WeftOS implementation.
4. Complete at least two successful tagged Cogs releases and a 30-day production soak with no unresolved extraction-related critical or high-severity defects.
5. Perform and record a rollback rehearsal to the retained WeftOS implementation and prior Cogs release.
6. Pass the documentation acceptance gate, including decision-id uniqueness, link checks, provenance, and public-repository hygiene.
7. Produce a Phase A proof report listing releases, deployments, tests, incidents, open defects, consumer migrations, documentation coverage, and rollback evidence.

Exit: the independent repository is proven in real use. WeftOS still contains the old implementation. The coordinator stops and requests explicit approval for Phase B.

## Phase B: remove the duplicate implementation from WeftOS

Phase B is a later project. It requires a separate board item, branch, review, and explicit user approval after the Phase A proof report. It must not begin as an automatic continuation of migration.

### B0: removal authorization

1. Review the Phase A proof report and every remaining consumer.
2. Confirm the 30-day soak, two tagged releases, rollback rehearsal, and absence of blocking defects.
3. Freeze the exact known-good Cogs and protocol versions WeftOS will consume.
4. Obtain explicit approval to remove the duplicated paths from WeftOS.

Exit: a recorded go/no-go decision. A no-go leaves WeftOS unchanged and extends the soak.

### B1: WeftOS removal

1. Remove the migrated Cogs product crates, generated web assets, catalog data, and product-specific build lanes from WeftOS in one independently reviewable removal series.
2. Retain the stable Cog protocol/client, generic workload primitives, thin `weaver cog` client commands, and boundary ADRs.
3. Replace moved operational documentation with short ownership documents and links to tagged Cogs documentation. Retain accepted WeftOS ADRs as immutable historical records with successor links; never delete or renumber them merely because implementation moved.
4. Prove a clean WeftOS checkout builds and tests without Cogs product sources, and a clean Cogs checkout builds and tests without WeftOS sources.
5. Run the complete WeftOS, Cogs, dashboard, deployment, leak, licence, dependency, and provenance gates again.
6. Keep the removal series revertible until the next stable WeftOS release and verify rollback from its release candidate.

Exit: WeftOS contains only the OS boundary and versioned Cogs client contract.

### B2: cleanup and branch retirement

1. Close or archive every migration-ledger branch only after its commit is reachable from a retained branch or signed archive.
2. Remove manual catalog-copy steps and obsolete deployment scripts.
3. Archive superseded worktrees after verifying their clean status and remote reachability.
4. Preserve the last pre-removal WeftOS tag and the migration receipts indefinitely.

Exit: no mutable source of truth or active branch is stranded in an old checkout.

## Validation gates

The destination repository must provide one command that runs:

- Rust formatting, check, clippy with warnings denied, unit tests, doctests, and feature-matrix checks;
- native Cog Manager and browser/WASM builds;
- Guide View and sensor scope WASM builds;
- Sensor Explorer TypeScript typecheck, lint, tests, Worker build, migration validation, and local D1 seed round trip;
- Cog conformance and contract checks;
- x86_64 Cog Host container build and armv7/aarch64 cross builds;
- catalog reproducibility and no-diff checks;
- signed registry and revocation negative tests;
- secret, private-path, client-name, generated-binary, license, and dependency scans.
- documentation build, link validation, unique decision-id validation, source-provenance checks, and stale-path checks.

Before Phase A can be declared proven, an integration lane must prove:

- WeftOS can list, resolve, install, start, stop, and revoke a Cog through the versioned boundary;
- Cog Host fails closed when the daemon, licence authority, signer, or revocation data is unavailable;
- Cog Manager and Sensor Explorer agree on item ids, guides, grades, versions, and artifact digests;
- dashboard launch links pass host/project context without putting durable tokens in URLs;
- rollback to the previous Cog Host and catalog release is documented and tested.

## Release and version policy

Recommend starting the expanded Cogs repository at `0.1.0` for its product release while independently versioning the wire protocol as `weftos.cog.v1`. Record the WeftOS import point as `weftos-0.8.3-import.1`. This avoids implying that every Cogs release shares the WeftOS release cadence.

All cross-repository dependencies use published versions or immutable tags. A two-pull-request cutover is required: publish the Cogs protocol/artifact first, then update WeftOS consumers. Never point a released WeftOS build at a mutable branch.

## Work lanes after approval

The coordinator session should delegate these lanes, each in an isolated branch/worktree with an adversarial reviewer:

1. **Inventory and history**: path map, branch ledger, dirty-work preservation, filtered imports.
2. **Protocol boundary**: daemon RPC, Cog Host decoupling, compatibility vectors.
3. **Workspace extraction**: Cargo workspace, build tooling, crate paths, feature matrix, and the prepared Weave Manager/Cog Host cherry-pick lane.
4. **Catalog and guides**: canonical schema, generation, Cogs/Manager/Explorer parity.
5. **Sensor Explorer and web**: Worker/D1 migration, website, MCP, WASM builds, previews.
6. **Public Cog branches**: reconcile active sensor worktrees and their guides/tests.
7. **Private/upstream review**: classify Cognitum branches without moving confidential content.
8. **CI and releases**: cross builds, provenance, SBOM, signing, deployment artifacts.
9. **Consumer migration**: WeftOS, dashboard, appliance/server deployments, developer projects.
10. **Security review**: trust boundaries, tokens/origins, USB input, signed catalogs, revocation, public-repo hygiene.
11. **Documentation**: ADR/COG decision ledger, successor records, platform-contract links, path repairs, contributor guide, operator migration, rollback, and ownership.
12. **Integration test**: dual-repo release candidate, hardware canary, rollback proof.

The coordinator owns the dependency graph and merges. Implementers do not change default branches directly. Review findings must include a failing case and are fixed before the dependent lane proceeds.

## Decisions requested

The defaults below are recommended:

1. **Destination**: expand existing public `weave-logic-ai/weftos-cogs`.
2. **Repository shape**: one Cogs domain workspace, including Sensor Explorer and web apps.
3. **Private boundary**: retain the private Cognitum repository as a separate private/upstream repository.
4. **Protocol**: daemon RPC through a small versioned `cog-protocol` client; no full kernel dependency.
5. **Catalog owner**: Cogs repository is canonical; Explorer database and Manager bundles are generated consumers.
6. **History**: merge filtered histories into an integration branch; do not rewrite existing repositories.
7. **Versioning**: Cogs `0.1.0`, protocol `v1`, import marker `weftos-0.8.3-import.1`.
8. **Two-stage cutover**: Phase A migrates and proves the new repository without deletions; Phase B removal needs a separate approval after two tagged releases, a 30-day soak, and a rollback rehearsal.
9. **Website**: Sensor Explorer, Manager web, and Guide View deploy from Cogs CI; dashboard remains external.
10. **Start condition**: begin only after WeftOS 0.8.3 is tagged and source work is checkpointed.

## Explicit non-goals

- No repository creation, history rewrite, deployment, branch deletion, or default-branch merge during planning.
- No Git submodules.
- No permanent path or Git dependency from Cogs back to a WeftOS checkout.
- No import of generated WASM packages, `node_modules`, target directories, secrets, backups, or private client records.
- No simultaneous rewrite of Cog behavior while moving its history; behavior changes follow the extraction in separate pull requests.
- No deletion of old paths until release, consumer, and rollback gates pass.
- No automatic transition from Phase A migration to Phase B removal.
- No deletion or silent renumbering of accepted WeftOS ADR history; ownership changes use successor links.
