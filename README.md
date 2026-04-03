# Telepath 🛰️
### Cross-platform remote audio recording & streaming

Telepath lets you capture audio from a remote machine and record it directly into your DAW over the network with accurate latency compensation. The typical use case is recording from a microphone or audio interface connected to a laptop while your DAW runs on a separate desktop machine.

![License](https://img.shields.io/badge/license-MIT-blue.svg)
![Language](https://img.shields.io/badge/language-Rust-orange.svg)
![Platforms](https://img.shields.io/badge/platforms-Windows%20%7C%20Linux-lightgrey.svg)

---

## 🚀 Overview

Telepath consists of two standalone applications:

1. **Host**: runs on the machine with the microphone / audio interface. Captures audio, streams it over the network, and can play back DAW output for monitoring.
2. **Receiver**: runs on the DAW machine. Receives the audio stream and routes it to a virtual audio device (VB-Cable on Windows, ALSA loopback on Linux), which your DAW records from as if it were a normal microphone input.

Optionally, the receiver can also capture DAW playback and stream it back to the host, so you can monitor what the DAW is playing through your audio interface while travelling.

```
[Microphone] → Host → network → Receiver → Virtual device → DAW (records)
[Scarlett out] ← Host ← network ← Receiver ← DAW loopback   (optional monitor)
```

No DAW plugin is required.

---

## 🛠️ Prerequisites

- [Rust toolchain](https://www.rust-lang.org/tools/install)
- **Windows (Receiver machine):** [VB-Cable](https://vb-audio.com/Cable/) + [FlexASIO](https://github.com/dechamps/FlexASIO) (see setup below)
- **Linux (Host machine):** No extra dependencies, ALSA is used directly

---

## 🔧 Building

Build both applications from the workspace root:

```bash
cargo build --release --bin telepath-host
cargo build --release --bin telepath-receiver
```

Binaries are written to `target/release/`.

---

## 🪟 Windows Setup (Receiver machine)

Cubase and most DAWs on Windows use ASIO exclusively and cannot see standard Windows audio devices. The following one-time setup makes VB-Cable visible to your DAW.

### 1. Install VB-Cable

Download and install from [vb-audio.com/Cable](https://vb-audio.com/Cable/). This creates a virtual loopback device, the receiver plays into it, and your DAW records from it.

### 2. Install FlexASIO

Download from [github.com/dechamps/FlexASIO](https://github.com/dechamps/FlexASIO). This exposes VB-Cable (and other Windows audio devices) as a proper ASIO driver.

### 3. Configure FlexASIO

Create `C:\Users\<your username>\FlexASIO.toml` with the following content. Use **FlexASIO GUI** (installed alongside FlexASIO) to find the exact device names on your system:

```toml
backend = "WASAPI"

[input]
device = "CABLE Output (VB-Audio Virtual Cable)"

[output]
device = "Speakers (Realtek High Definition Audio)"  # your PC's speakers or output
```

### 4. Configure Cubase

- Studio → Studio Setup → VST Audio System → set ASIO driver to **FlexASIO**
- Studio → Audio Connections → Inputs → add a new bus mapped to **CABLE Output**
- Set your recording track's input to that bus

---

## 🎙️ Usage

### Basic recording session

**On the Host machine (with the microphone):**
1. Run `telepath-host`
2. Select your microphone input device
3. Optionally select an output device (e.g. your Scarlett headphone output) for DAW monitoring

**On the Receiver machine (running the DAW):**
1. Run `telepath-receiver`
2. Select **CABLE Input (VB-Audio Virtual Cable)** as the output device
3. Enter the Host machine's IP address and click **Connect**
4. Click **⏺ REC** in the host application to start streaming
5. Record-arm your DAW track and hit record

### Latency compensation

After connecting, the receiver displays the measured one-way network latency (e.g. `12.34 ms`). Enter this value as a fixed track delay in your DAW:

- **Cubase:** right-click the track → Track Delay → enter the value in milliseconds
- **Reaper:** track properties → Track delay

This shifts the recorded audio back into alignment with your timeline automatically.

### DAW monitor return stream (optional)

To hear DAW playback through your audio interface while on the road:

1. On the **Receiver**, select a return stream input device. Typically "Stereo Mix" or "What U Hear" (exposed by most Realtek drivers), or a second VB-Cable instance set to capture FlexASIO's output
2. Click **▶ Send DAW audio to host**
3. On the **Host**, select your audio interface output and click **▶ Monitor DAW**

DAW audio will now play through your interface headphone output in real time.

---

## 📡 Network

Both machines must be on the same network (LAN or VPN). Telepath uses:

- **TCP port 7271**: control channel (handshake, ping/RTT, stats)
- **UDP**: audio stream (port negotiated during handshake)

If the host machine is behind a router, forward TCP port 7271 and ensure UDP traffic is not blocked. A VPN (e.g. Tailscale) is the easiest option for connecting across different networks without port forwarding.

---

## 🏗️ Project Structure

```
telepath/
├── host/       # Capture + streaming application (runs on the mic machine)
├── receiver/   # Playback application (runs on the DAW machine)
└── common/     # Shared protocol types, packet encoding, TCP framing
```
