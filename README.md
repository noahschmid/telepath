# Telepath 🛰️
### Cross-platform remote audio recording & streaming

Telepath is a cross-platform utility that bridges the gap between remote audio sources and your DAW. It captures system audio from a host machine and streams it directly into a plugin instance over the network. This allows you to work on audio projects outside the confines of your local setup, making it ideal for remote work scenarios.

![License](https://img.shields.io/badge/license-MIT-blue.svg)
![Language](https://img.shields.io/badge/language-Rust-orange.svg)
![Formats](https://img.shields.io/badge/formats-CLAP%20%7C%20VST3-green.svg)
![Platforms](https://img.shields.io/badge/platforms-Windows%20%7C%20macOS%20%7C%20Linux-lightgrey.svg)

---

## 🚀 Overview
The Telepath system consists of two components:
1.  **The Host Application:** Captures system audio from any computer (Windows, macOS, Linux).
2.  **The Plugin:** Receives the stream inside your DAW (CLAP or VST3).

---

## 🛠️ Installation

### Prerequisites
Ensure you have the [Rust toolchain](https://www.rust-lang.org/tools/install) installed on your system.

### Host Application
The host application runs on Windows, macOS and Linux. You can compile and run it using

```bash
cargo run --bin telepath-host --release
```

### Plugin Installation
First run 

```bash
cargo xtask bundle telepath --release
```

Then copy the generated plugin files to the appropriate directories for your system. Example for Linux:

Clap:
```bash
cp -r target/bundled/telepath.clap ~/.clap/
```

VST:
```bash
cp -r target/bundled/telepath.vst3 ~/.vst3/
```

## Usage
1. Start the host application on the computer you want to record from.
2. Open your DAW, create a new track and load the telepath plugin.
3. Enter the IP address of the host application in the plugin settings.
4. Hit the recording button in the host application to start streaming audio to the plugin.
