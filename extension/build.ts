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

// Registered as a MAIN-world content script (see capture-registration.ts),
// which loads it as a plain script, not a module, so it must not emit any
// import/export statements.
const captureBundle = await Bun.build({
  entrypoints: ["src/instagram-capture.ts"],
  minify: false,
  outdir: outputDirectory,
  target: "browser",
  format: "iife",
});

if (!captureBundle.success) {
  for (const message of captureBundle.logs) {
    console.error(message);
  }
  process.exit(1);
}

for (const fileName of ["manifest.json", "options.html", "options.css"]) {
  await Bun.write(`${outputDirectory}/${fileName}`, Bun.file(`src/${fileName}`));
}
