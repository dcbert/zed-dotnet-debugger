// Cargo test runner for wasm32-wasip1 builds (no C toolchain / wasmtime needed): `node` provides WASI preview1.
import { WASI } from "node:wasi";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const [wasmPath, ...args] = process.argv.slice(2);
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "dotnet-debugger-test-"));
const wasi = new WASI({
  version: "preview1",
  args: [wasmPath, ...args],
  env: { ...process.env, DD_SCRATCH: "/tmp" },
  preopens: { "/tmp": tmp },
  returnOnExit: true,
});
const module = await WebAssembly.compile(fs.readFileSync(wasmPath));
const instance = await WebAssembly.instantiate(module, wasi.getImportObject());
let code = 1;
try {
  code = wasi.start(instance);
} finally {
  fs.rmSync(tmp, { recursive: true, force: true });
}
process.exit(code);
