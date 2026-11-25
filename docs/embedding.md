# Choosing how Benedict embeds

Semantic search lets a search find pages that say the same thing in other words. It is optional: Benedict always answers with word search, and semantic search is fused with it when it is on. This page helps you decide whether to turn it on and where the embedding model should run.

## What it adds and what it costs

| | Word search only | With semantic search |
| --- | --- | --- |
| Finds | Pages that share the question's words | Also pages that use other words for the same thing |
| Measured effect | — | On two libraries with 18 and 13 real questions, the right page came first for 5 instead of 1, and 3 instead of 2 |
| Download | Nothing | The model file once (558 MB, SHA-256 checked) |
| Memory | Nothing more | About 0.7 GB for the model server (2 GiB limit in the container) |
| Work | Nothing more | Every new or changed document is embedded once, in 1,000-character parts |
| Data | Stays on the machine | Stays on the machine, unless you point it at a server elsewhere |

The model is EmbeddingGemma 2 (768 dimensions). Vectors from one model name are not mixed with another's: changing the model name embeds every document again.

## The four choices

| Choice | How to choose it | Good for |
| --- | --- | --- |
| Off | `--semantic off` | Small machines, PostgreSQL installations, or when word search is enough |
| Embedder container (default when it fits) | nothing, or `--semantic on` | Linux servers; small wikis anywhere |
| Embedding server on the host | `--embed-url http://HOST:PORT` | Macs and machines with a GPU; large wikis |
| Embedding server elsewhere | `--embed-url http://HOST:PORT` | Several libraries sharing one GPU machine on your network |

The installer turns semantic search on by itself only when the 2 GiB embedder container fits beside the library in two thirds of Docker's memory, and shows the choice and the reason in its summary. Every choice can be changed later on the same project:

```bash
bin/install-project --semantic off PROJECT                       # word search only
bin/install-project --semantic on PROJECT                        # the embedder container
bin/install-project --embed-url http://HOST:PORT PROJECT         # a server that already runs
```

A run with the image that last indexed the project keeps the index. Check the result with `PROJECT/.benedict/library search "a question"`: `engine` is `hybrid-rrf` and `semantic.status` is `ok` when every document has vectors, `partial` while some still wait (the librarian embeds up to 2,000 parts a cycle).

## The embedder container and where it is slow

The default embedder is llama.cpp's server in a container on two CPU cores, without a GPU. On Linux the container runs on the host itself, so this is as fast as the machine's CPU.

Docker Desktop on macOS and Windows, and Colima, run containers in a virtual machine that cannot use the host's GPU. There the container is much slower. Measured on an Apple silicon Mac with the same model file:

| | Embedder container (2 CPU cores in the VM) | Server on the host (GPU) |
| --- | --- | --- |
| One 1,000-character part | 14.06 s | 0.04 s |
| One question | 0.42 s | 0.012 s |
| Questions asked while documents are being embedded (2 s wait) | 10 of 10 took longer; search used words only | 0 of 10 |
| Peak memory / CPU | 913 MiB / two cores full | 656 MiB / 10 % of one core |
| Vectors | — | the same (cosine 1.00000 over 36 texts) |

A wiki of a few hundred pages is fine either way. For thousands of pages on a Mac, run the server on the host.

## Running the embedding server on the host

Use the same server and model file as the container, so the vectors match it:

- Server: llama.cpp `llama-server` (Homebrew `brew install llama.cpp` on macOS, or a CUDA build on Linux). Use build b11393 or later, which fixes CVE-2026-107183, and do not pass `--sleep-idle-seconds` (CVE-2026-43631, CVE-2026-43632).
- Model: `embeddinggemma-2-BF16.gguf` from `https://huggingface.co/ggml-org/embeddinggemma-2-GGUF` at revision `6ea3e3a309d12a4779cfda98b8c19800d2d5da71`, SHA-256 `68bae29d62fb8c7d23e98d21fd4662753ddd636e6b62b8a70dfe059a9844f216`. If the embedder container already downloaded it, copy it out of the `library_models` volume.
- Command:

```bash
llama-server -m /path/to/embeddinggemma-2-BF16.gguf --embedding --pooling mean \
  -c 2048 -b 2048 -ub 2048 -ngl 99 --host ADDRESS --port 18080
```

`-ngl 99` puts the model on the GPU (Metal on Apple silicon, CUDA on NVIDIA). `ADDRESS` must be reachable from the library's containers and nothing wider:

| Docker | Address the containers use for the host | What to bind |
| --- | --- | --- |
| Docker Desktop (macOS, Windows) | `host.docker.internal` | `127.0.0.1` |
| Colima with `--network-address` | the VM network's gateway on the host (for example the `bridge100` address) | that gateway address |
| Colima without a network address | `host.lima.internal` | `127.0.0.1` |
| Docker Engine on Linux | the bridge gateway (`docker network inspect`, usually `172.17.0.1`), allowed through the firewall | that gateway address |

Colima's user-mode network stalled single connections for over two seconds after a few hundred requests in a row; the librarian waits and sends again, but a VM network address avoids the stalls.

Then point the library at it. To keep the vectors the embedder container already made, give the same model name:

```bash
bin/install-project --embed-url http://ADDRESS:18080 --embed-model embeddinggemma-2-BF16@6ea3e3a PROJECT
```

The installer removes the embedder container. Without `--embed-model`, the model name is `embeddinggemma-2` and every document is embedded again.

### Keeping it running

macOS (`launchd`): save as `~/Library/LaunchAgents/org.benedict.embedder.plist` to start it when you log in, or as a system daemon in `/Library/LaunchDaemons` with a `UserName` key to start it at boot without a login. Load it with `launchctl bootstrap gui/$(id -u) FILE` (or `sudo launchctl bootstrap system FILE`).

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>org.benedict.embedder</string>
  <key>ProgramArguments</key>
  <array>
    <string>/opt/homebrew/bin/llama-server</string>
    <string>-m</string><string>/path/to/embeddinggemma-2-BF16.gguf</string>
    <string>--embedding</string><string>--pooling</string><string>mean</string>
    <string>-c</string><string>2048</string><string>-b</string><string>2048</string><string>-ub</string><string>2048</string>
    <string>-ngl</string><string>99</string>
    <string>--host</string><string>ADDRESS</string><string>--port</string><string>18080</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ThrottleInterval</key><integer>30</integer>
</dict>
</plist>
```

Linux (`systemd`): `/etc/systemd/system/benedict-embedder.service`, then `sudo systemctl enable --now benedict-embedder`.

```ini
[Unit]
Description=Embedding server for Benedict
After=network-online.target docker.service

[Service]
ExecStart=/usr/local/bin/llama-server -m /path/to/embeddinggemma-2-BF16.gguf --embedding --pooling mean -c 2048 -b 2048 -ub 2048 -ngl 99 --host ADDRESS --port 18080
Restart=always
RestartSec=30
User=YOUR_USER

[Install]
WantedBy=multi-user.target
```

## An embedding server elsewhere

`--embed-url` takes any server that answers `POST /v1/embeddings` over plain HTTP. The text of every document and question is sent to it, so keep it on a network you trust. Name its model with `--embed-model`; libraries that share a server and a model name can share vectors computed the same way.

## When something fails

Semantic search never stops the library. If the server does not answer, a search returns the word results with `semantic.status` `failed` and the reason, and exits with status 1; the librarian's `vectors` step reports `failed` and tries again next cycle. A slow server is waited on as long as its own records say (see `benedict_embed_pace` in [portability.md](portability.md)).

