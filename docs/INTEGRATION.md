# Embedding in Fieldwork

Fieldwork's **Import Drill** action should call `libthree_du`, the C ABI in
`crates/three-du-ffi`. It runs exactly the same code as the CLI. The header is
`crates/three-du-ffi/include/three_du.h`.

## API

| Function | Purpose |
|---|---|
| `tdu_version()` | Library version (borrowed string) |
| `tdu_detect_format(data, len)` | `TDU_FORMAT_*` by content |
| `tdu_open_path(path, progress, ctx, &err)` | Decode a `.3du` from disk (read-only) |
| `tdu_open_bytes(data, len, progress, ctx, &err)` | Decode from memory (buffer is copied) |
| `tdu_project_summary_json(p)` | Counts, versions, diagnostics; no names (safe to log) |
| `tdu_project_json(p)` | Full intermediate model (contains drill data) |
| `tdu_export(p, format, path, flags, &diagnostics, &err)` | Write `.fieldwork`, `.dots`, `.om`, `.omz` or JSON atomically |
| `tdu_error_code`, `tdu_error_message` | Inspect an error |
| `tdu_project_free`, `tdu_string_free`, `tdu_error_free` | Release what the library returned |

Export formats include `TDU_FORMAT_MUSICXML` (count track on the drill's clock).

Flags: `TDU_FLAG_STRICT`, `TDU_FLAG_ALLOW_LOSSY`, `TDU_FLAG_OVERWRITE`, `TDU_FLAG_PRESERVE_CURVES`.

Error codes are stable (`TDU_ERR_NOT_3DU`, `TDU_ERR_UNSUPPORTED_VERSION`, `TDU_ERR_CORRUPT`,
`TDU_ERR_CANCELLED`, `TDU_ERR_REFUSED_LOSSY`, `TDU_ERR_OUTPUT_EXISTS`, …).

## Rules at the boundary

- **Ownership**: every returned pointer is owned by the caller and freed once with the matching
  `tdu_*_free`. Never use `free()`. `tdu_version()` and `tdu_error_message()` are borrowed.
- **Errors**: failing calls return NULL or a nonzero code and fill `*out_error` if the pointer
  is non-NULL. Messages never contain drill names.
- **Threads**: a `TduProject` is immutable and may be shared across threads. Run decoding off
  the main thread; the progress callback runs on the decoding thread.
- **Cancellation**: return `false` from the progress callback; decoding stops with `TDU_ERR_CANCELLED`.
- **Panics** are caught and reported as `TDU_ERR_INTERNAL`.

## Swift (macOS)

```swift
import ThreeDU   // module map over three_du.h

func importDrill(from url: URL, to output: URL) throws -> String {
    var error: OpaquePointer?
    guard let project = tdu_open_path(url.path, nil, nil, &error) else {
        defer { tdu_error_free(error) }
        throw ImportError(code: tdu_error_code(error), message: String(cString: tdu_error_message(error)))
    }
    defer { tdu_project_free(project) }
    var diagnostics: UnsafeMutablePointer<CChar>?
    let rc = tdu_export(project, Int32(TDU_FORMAT_FIELDWORK), output.path, 0, &diagnostics, &error)
    guard rc == TDU_OK else {
        defer { tdu_error_free(error) }
        throw ImportError(code: rc, message: String(cString: tdu_error_message(error)))
    }
    defer { tdu_string_free(diagnostics) }
    return String(cString: diagnostics!)  // JSON array to show as import warnings
}
```

Swift imports `TduProject *` / `TduError *` as `OpaquePointer`. Pass a closure to `progress`
via `@convention(c)` and a context pointer to report progress and support cancellation.

### Building an XCFramework

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin
cargo build --release -p three-du-ffi --target aarch64-apple-darwin
cargo build --release -p three-du-ffi --target x86_64-apple-darwin
lipo -create target/aarch64-apple-darwin/release/libthree_du.a \
             target/x86_64-apple-darwin/release/libthree_du.a -output build/libthree_du.a

mkdir -p build/include
cp crates/three-du-ffi/include/three_du.h build/include/
cat > build/include/module.modulemap <<'EOF'
module ThreeDU {
    header "three_du.h"
    link "three_du"
    export *
}
EOF

xcodebuild -create-xcframework -library build/libthree_du.a -headers build/include \
           -output build/ThreeDU.xcframework
```

Add `ThreeDU.xcframework` to the Fieldwork target. The static library bundles SQLite, so
nothing else needs linking, apart from `libc++`/`libiconv` if the linker asks.

## Windows

`cargo build --release -p three-du-ffi` produces:

- `three_du.dll` with import library `three_du.dll.lib`, for dynamic linking;
- `three_du.lib`, a static library;
- `libthree_du.rlib`, for Rust callers.

Fieldwork for Windows is Swift, so it can use the same module map with the DLL. Ship
`three_du.dll` next to the executable. A C smoke test that compiles against the header
and links the DLL has been run on Windows with clang.

## Suggested import flow

1. `tdu_detect_format` on the file the user picked. If it isn't `TDU_FORMAT_3DU`, explain which
   format it is.
2. `tdu_open_path` on a background task, with progress.
3. Show `tdu_project_summary_json`: performers, sets, warnings. Offer "keep curves exact"
   (`TDU_FLAG_PRESERVE_CURVES`).
4. `tdu_export(…, TDU_FORMAT_FIELDWORK, temp path, …)`, then open the result with Fieldwork's
   own `DocumentFile.open`.
