export {};

const outputDirectory = "dist";

const bundle = await Bun.build({
  entrypoints: ["src/service-worker.ts", "src/options.ts"],
  minify: false,
  outdir: outputDirectory,
  target: "browser",
});

if (!bundle.success) {
  for (const message of bundle.logs) {
    console.error(message);
  }
  process.exit(1);
}

for (const fileName of ["manifest.json", "options.html", "options.css"]) {
  await Bun.write(`${outputDirectory}/${fileName}`, Bun.file(`src/${fileName}`));
}
