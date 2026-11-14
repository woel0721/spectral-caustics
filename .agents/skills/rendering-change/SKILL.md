---
name: rendering-change
description: Use when changing or implementing camera or photon rendering behavior, BSDFs, lighting, sampling, MIS, NEE, path continuation, geometry that affects transport, or the current homogeneous-medium estimator. Do not use as the primary workflow when wavelength or spectral integration is the main subject.
---

# Rendering Change

Use this workflow for a scoped change to the renderer's existing estimators. The goal is the smallest implementation that preserves the project's physical contracts.

## Read first

- `.agents/architecture.md` for ownership and execution flow.
- `.agents/transport.md` for estimator, probability-measure, throughput and contribution contracts.
- `.agents/testing.md` for the change-to-regression map.
- Read `.agents/project.md` when changing validation, error handling, dependencies or repository conventions.

If wavelength sampling, spectral reconstruction, dispersion or XYZ/Eλ integration is the main subject, use `spectral-change` instead. If the task primarily changes module ownership or removes an implementation, use `architecture-refactor`.

## Workflow

1. **Locate the actual owner.** Trace the public/high-level entry point to the code that owns the estimator. Inspect nearby implementations and callers; do not infer behavior from names or README text alone.
2. **State the estimator.** Identify what quantity is estimated, where the contribution is accumulated (Direct or Caustics), the sampling strategy and its measure (solid angle, area or discrete event), and which PDF/weight terms are already compensated.
3. **Identify invariants before editing.** Check relevant direction conventions, cosine factors, visibility/transmittance, MIS quota, path budgets, RR, RNG stream ownership, linear XYZ accumulation and deterministic reduction.
4. **Choose the smallest change.** Reuse an existing abstraction only when it represents the same physical operation and ownership. Keep camera-radiance and photon-flux semantics distinct. Do not add a generic framework or compatibility layer for hypothetical future use.
5. **Implement and test.** Add or update focused mathematical/behavioral tests at the estimator boundary. Keep independent reference calculations where they validate the implementation rather than copy it.
6. **Verify the affected pipeline.** Use the relevant entries in `.agents/testing.md`. If rendered appearance changes, run a representative low-cost render in a separate output location and record configuration, seed, mode and SPP. Do not render unrelated scenes by default.
7. **Review the diff.** Check for unrelated edits, duplicated compensation, stale comments and unnecessary validation. Run `git diff --check`.

## Important boundaries

- A BSDF sample weight may already include the scattering value, cosine and PDF compensation. Follow the contract; never apply it twice.
- Do not treat a discrete Delta event as an ordinary solid-angle PDF.
- Do not average a fixed-quota MIS sum a second time or silently replace rejected samples.
- Do not add a second environment contribution to a miss already owned by local NEE/MIS.
- Do not read the caustics map at ineligible later ground vertices or add another irradiance/cosine factor to a map that already stores irradiance.
- A phase-function sample has no surface cosine.
- Preserve legitimate zero contribution, TIR, degenerate direction and non-finite/overflow handling. Remove checks only when a stronger invariant demonstrably guarantees the condition.
- Keep fixed-seed behavior and Rayon 1/2/4 determinism for unchanged settings. Do not promise old-version RNG or PNG identity.

## Completion report

Summarize the changed behavior and owning modules, tests/checks actually run, render parameters if applicable, and any checks not run. Distinguish mathematical test evidence from visual inspection.
