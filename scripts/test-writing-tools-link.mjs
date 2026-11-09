// Rust must link Clang's @available runtime itself. A clang-linked UI test (or
// Rust std's own availability implementation) can otherwise hide this failure.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, mkdir, readFile, writeFile, copyFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { spawnSync } from 'node:child_process'

test('production macOS native module links with Rust release LTO for ARM64 and Intel', { skip: process.platform !== 'darwin', timeout: 120000 }, async () => {
  const fixture = await mkdtemp(join(tmpdir(), 'argus-writing-tools-link-'))
  try {
    const lock = await readFile(new URL('../src-tauri/Cargo.lock', import.meta.url), 'utf8')
    const ccVersion = lock.match(/\[\[package\]\]\nname = "cc"\nversion = "([^"]+)"/)[1]
    await mkdir(join(fixture, 'src'))
    await writeFile(join(fixture, 'Cargo.toml'), `[package]
name = "argus-writing-tools-link-test"
version = "0.0.0"
edition = "2021"
[lib]
crate-type = ["staticlib", "cdylib", "rlib"]
[build-dependencies]
cc = "=${ccVersion}"
[profile.release]
lto = "thin"
codegen-units = 1
strip = true
panic = "abort"
`)
    // Use the actual production build script, stubbing only unrelated Tauri
    // configuration generation. Native compiler discovery/link directives run.
    const build = await readFile(new URL('../src-tauri/build.rs', import.meta.url), 'utf8')
    await writeFile(join(fixture, 'build.rs'), 'mod tauri_build { pub fn build() {} }\n' + build)
    await copyFile(new URL('../src-tauri/src/macos_writing_tools.m', import.meta.url), join(fixture, 'src/macos_writing_tools.m'))
    await writeFile(join(fixture, 'src/lib.rs'), `#![no_std]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! { loop {} }
#[link(name = "System")]
#[link(name = "objc")]
#[link(name = "Foundation", kind = "framework")]
extern "C" { fn argus_writing_tools_menu(); }
pub fn native_entrypoint() -> unsafe extern "C" fn() { argus_writing_tools_menu }
`)
    await writeFile(join(fixture, 'src/main.rs'), `#![no_std]
#![no_main]
#[no_mangle]
pub extern "C" fn main() -> i32 {
    core::hint::black_box(argus_writing_tools_link_test::native_entrypoint());
    0
}
`)
    for (const target of ['aarch64-apple-darwin', 'x86_64-apple-darwin']) {
      const result = spawnSync('cargo', ['build', '--manifest-path', join(fixture, 'Cargo.toml'), '--release', '--target', target, '--offline'], {
        encoding: 'utf8', timeout: 55000, env: { ...process.env, CARGO_TARGET_DIR: join(fixture, 'target') },
      })
      assert.equal(result.status, 0, `${target}: ${result.error ?? ''}\n${result.stderr.slice(-10000)}`)
      console.log(`${target}: native Objective-C module linked by Rust with release LTO and no std availability fallback`)
    }
  } finally { await rm(fixture, { recursive: true, force: true }) }
})
