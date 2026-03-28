## Installation
First run 

```bash
cargo xtask bundle telepath --release
```

Then copy the generated plugin files to the appropriate directories for your system.

Clap:
```bash
cp -r target/bundled/telepath.clap ~/.clap/
```

VST:
```bash
cp -r target/bundled/telepath.vst3 ~/.vst3/
```
