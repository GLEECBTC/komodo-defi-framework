# Building WASM binary

## From Container:

If you want to build from source without installing prerequisites to your host system, you can do so by binding the source code inside a container and compiling it there.

Build the image:

```sh
docker build -t kdf-build-container -f .docker/Dockerfile .
```

Bind source code into container and compile it:
```sh
docker run -v "$(pwd)":/app -w /app kdf-build-container wasm-pack build mm2src/mm2_bin_lib --target web --out-dir wasm_build/deps/pkg/
```

## Setting up the environment

To build WASM binary from source, the following prerequisites are required:

1. Install `wasm-pack`
   ```
   curl https://rustwasm.github.io/wasm-pack/installer/init.sh -sSf | sh
   ```
2. OSX specific: install `llvm`
   ```
   brew install llvm
   ```

## Compiling WASM release binary

To build WASM release binary run one of the following commands according to your environment:

- for Linux users:
   ```
   wasm-pack build mm2src/mm2_bin_lib --target web --out-dir wasm_build/deps/pkg/
   ```
- for OSX users (Intel):
   ```
   CC=/usr/local/opt/llvm/bin/clang AR=/usr/local/opt/llvm/bin/llvm-ar wasm-pack build mm2src/mm2_bin_lib --target web --out-dir wasm_build/deps/pkg/
   ```
- for OSX users (Apple Silicon):
   ```
   CC=/opt/homebrew/opt/llvm/bin/clang AR=/opt/homebrew/opt/llvm/bin/llvm-ar wasm-pack build mm2src/mm2_bin_lib --target web --out-dir wasm_build/deps/pkg/
   ```

Please note `CC` and `AR` must be specified in the same line as `wasm-pack test mm2src/mm2_main`.

## Compiling WASM binary with debug symbols

If you want to disable optimizations to reduce the compilation time, run `wasm-pack build mm2src/mm2_bin_lib` with an additional `--dev` flag:
```
wasm-pack build mm2src/mm2_bin_lib --target web --out-dir wasm_build/deps/pkg/ --dev
```

Please don't forget to specify `CC` and `AR` if you run the command on OSX.


