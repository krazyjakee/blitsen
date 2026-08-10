const HELP = `Usage: blitsen <command> [options]

Commands:
  dev <directory>   Watch static output and reload on changes
  build <directory> Build an exported application
  doctor <directory> Report unsupported web APIs

The runtime commands are scaffolded but not implemented yet.`;

export function main(args, output = console) {
  if (args.length === 0 || args.includes("--help") || args.includes("-h")) {
    output.log(HELP);
    return 0;
  }
  if (args.includes("--version") || args.includes("-v")) {
    output.log("0.0.0");
    return 0;
  }
  output.error(`blitsen: ${args[0]} is not implemented yet`);
  return 1;
}
