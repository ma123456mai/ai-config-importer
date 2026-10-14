import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const manifest = JSON.parse(fs.readFileSync(path.join(root, "manifest.json"), "utf8"));
const outputDirectory = path.join(root, "dist");
const packagePrefix = "ai-config-importer";
const sourcePrefix = `${manifest.id}-${manifest.version}-`;

const packageFiles = fs.readdirSync(outputDirectory)
  .filter((name) => name.startsWith(sourcePrefix) && name.endsWith(".dbxp"));
const metadataFiles = fs.readdirSync(outputDirectory)
  .filter((name) => name.startsWith(sourcePrefix) && name.endsWith(".artifact.json"));

if (packageFiles.length !== 1 || metadataFiles.length !== 1) {
  throw new Error(`Expected one generated package and metadata file for ${sourcePrefix}; found ${packageFiles.length} packages and ${metadataFiles.length} metadata files`);
}

const metadataPath = path.join(outputDirectory, metadataFiles[0]);
const metadata = JSON.parse(fs.readFileSync(metadataPath, "utf8"));
if (!/^[a-z0-9-]{1,64}$/.test(metadata.target || "")) {
  throw new Error(`Invalid artifact target: ${metadata.target}`);
}

const sourcePackageName = path.basename(new URL(metadata.url, "https://plugins.invalid/releases/").pathname);
if (sourcePackageName !== packageFiles[0]) {
  throw new Error(`Artifact metadata references ${sourcePackageName}, expected ${packageFiles[0]}`);
}

const renamedPackageName = `${packagePrefix}-${manifest.version}-${metadata.target}.dbxp`;
const renamedMetadataName = `${packagePrefix}-${manifest.version}-${metadata.target}.artifact.json`;
const sourcePackagePath = path.join(outputDirectory, packageFiles[0]);
const renamedPackagePath = path.join(outputDirectory, renamedPackageName);
const renamedMetadataPath = path.join(outputDirectory, renamedMetadataName);

metadata.url = renamedPackageName;
fs.renameSync(sourcePackagePath, renamedPackagePath);
fs.writeFileSync(renamedMetadataPath, `${JSON.stringify(metadata, null, 2)}\n`);
fs.rmSync(metadataPath);
console.log(`Renamed release artifacts to ${renamedPackageName} and ${renamedMetadataName}`);
