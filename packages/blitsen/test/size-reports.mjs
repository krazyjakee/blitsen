const bytes = value => `${(value / 1_000_000).toFixed(1)} MB`;

export function phase2SizeSummary(record) {
  const components = record.components;
  const rows = [
    ["Phase 2 bare export", record.phase2.bytes, record.phase2.gzip],
    ["Phase 1 bare export", record.phase1.bytes, record.phase1.gzip],
    ["runtime executable", components.runtimeExecutable, null],
    ["application payload", components.appPayload, null],
  ].map(([label, installed, compressed]) =>
    `| ${label} | ${bytes(installed)} | ${compressed === null ? "—" : bytes(compressed)} |`);
  return [
    `### Phase 2 bare-app size — ${record.platform}`,
    "",
    `Commit \`${record.commit ?? "working tree"}\`; runtime pinned by \`BLITSEN_RUNTIME_PATH\`.`,
    "",
    "| measurement | installed | gzip-9 |",
    "| --- | ---: | ---: |",
    ...rows,
    "",
    `Phase 2 is **${record.ratio}× smaller** than Phase 1 on this runner. `
      + "This is report-only; the regression gate remains separate.",
  ].join("\n");
}
