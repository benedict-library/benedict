# 베네딕트 임베딩 고르기

의미 검색은 같은 뜻을 다른 말로 쓴 페이지도 찾게 합니다. 고를 수 있는 기능입니다. 베네딕트는 늘 낱말 검색으로 답하고, 의미 검색을 켜면 두 결과를 합칩니다. 이 문서는 켤지 말지, 켠다면 임베딩 모델을 어디서 돌릴지 정하는 데 씁니다.

## 얻는 것과 드는 것

| | 낱말 검색만 | 의미 검색을 켜면 |
| --- | --- | --- |
| 찾는 것 | 질문과 같은 낱말이 든 페이지 | 같은 뜻을 다른 말로 쓴 페이지까지 |
| 잰 효과 | — | 실제 질문 18개·13개인 도서관 두 곳에서 정답 페이지가 첫째로 나온 수가 1→5, 2→3 |
| 내려받기 | 없음 | 모델 파일 한 번(558MB, SHA-256 확인) |
| 메모리 | 더 들지 않음 | 모델 서버 약 0.7GB(컨테이너 상한 2GiB) |
| 일 | 더 들지 않음 | 새 문서·바뀐 문서를 1,000자 조각으로 한 번씩 임베딩 |
| 자료 | 그 기계 안에 있음 | 그 기계 안에 있음(다른 곳의 서버를 지정하지 않는 한) |

모델은 EmbeddingGemma 2(768차원)입니다. 모델 이름이 다른 벡터는 섞지 않으므로, 모델 이름을 바꾸면 모든 문서를 다시 임베딩합니다.

## 네 가지 선택

| 선택 | 고르는 법 | 어울리는 곳 |
| --- | --- | --- |
| 끔 | `--semantic off` | 작은 기계, PostgreSQL 설치, 낱말 검색으로 충분할 때 |
| 임베더 컨테이너(들어갈 자리가 있으면 기본) | 아무것도 안 주거나 `--semantic on` | 리눅스 서버, 어디서든 작은 위키 |
| 그 기계 본체의 임베딩 서버 | `--embed-url http://HOST:PORT` | 맥, GPU가 있는 기계, 큰 위키 |
| 다른 곳의 임베딩 서버 | `--embed-url http://HOST:PORT` | 내부망의 GPU 기계 하나를 여러 도서관이 함께 쓸 때 |

설치기는 2GiB 임베더 컨테이너가 도서관과 함께 도커 메모리의 3분의 2 안에 들어갈 때만 스스로 의미 검색을 켜고, 고른 것과 이유을 요약에 보입니다. 같은 프로젝트에서 언제든 바꿀 수 있습니다.

```bash
bin/install-project --semantic off PROJECT                       # 낱말 검색만
bin/install-project --semantic on PROJECT                        # 임베더 컨테이너
bin/install-project --embed-url http://HOST:PORT PROJECT         # 이미 떠 있는 서버
```

마지막으로 색인한 이미지와 같은 이미지로 돌리면 색인은 그대로 둡니다. 결과는 `PROJECT/.benedict/library search "질문"`으로 봅니다. 모든 문서에 벡터가 있으면 `engine`이 `hybrid-rrf`, `semantic.status`가 `ok`이고, 아직 기다리는 문서가 있으면 `partial`입니다(사서가 한 주기에 2,000조각까지 임베딩).

## 임베더 컨테이너가 느린 곳

기본 임베더는 GPU 없이 CPU 2코어로 도는 컨테이너 안의 llama.cpp 서버입니다. 리눅스에서는 컨테이너가 본체에서 바로 돌아 그 기계 CPU만큼 빠릅니다.

맥·윈도우의 Docker Desktop과 Colima는 컨테이너를 가상 기계 안에서 돌리고, 그 가상 기계는 본체 GPU를 쓰지 못해 훨씬 느립니다. 같은 모델 파일로 애플 실리콘 맥에서 잰 값입니다.

| | 임베더 컨테이너(가상 기계 안 CPU 2코어) | 본체 서버(GPU) |
| --- | --- | --- |
| 1,000자 조각 하나 | 14.06초 | 0.04초 |
| 질문 하나 | 0.42초 | 0.012초 |
| 문서 임베딩 중에 한 질문(2초 대기) | 10개 모두 넘김 — 낱말로만 검색 | 0개 |
| 최고 메모리 / CPU | 913MiB / 2코어 꽉 참 | 656MiB / 한 코어의 10% |
| 벡터 | — | 같음(36개 글 코사인 1.00000) |

수백 페이지 위키는 어느 것을 써도 괜찮습니다. 맥에서 수천 페이지라면 본체 서버를 쓰십시오.

## 본체에서 임베딩 서버 돌리기

벡터가 컨테이너와 같도록 같은 서버와 같은 모델 파일을 씁니다.

- 서버: llama.cpp `llama-server`(맥은 Homebrew `brew install llama.cpp`, 리눅스는 CUDA 빌드). CVE-2026-107183을 고친 b11393 이후 빌드를 쓰고, `--sleep-idle-seconds`는 주지 마십시오(CVE-2026-43631, CVE-2026-43632).
- 모델: `https://huggingface.co/ggml-org/embeddinggemma-2-GGUF`의 리비전 `6ea3e3a309d12a4779cfda98b8c19800d2d5da71`에 있는 `embeddinggemma-2-BF16.gguf`, SHA-256 `68bae29d62fb8c7d23e98d21fd4662753ddd636e6b62b8a70dfe059a9844f216`. 임베더 컨테이너가 이미 받았다면 `library_models` 볼륨에서 꺼내 씁니다.
- 명령:

```bash
llama-server -m /path/to/embeddinggemma-2-BF16.gguf --embedding --pooling mean \
  -c 2048 -b 2048 -ub 2048 -ngl 99 --host ADDRESS --port 18080
```

`-ngl 99`는 모델을 GPU에 올립니다(애플 실리콘은 Metal, 엔비디아는 CUDA). `ADDRESS`는 도서관 컨테이너가 닿는 주소여야 하고, 그보다 넓게 열지 않습니다.

| 도커 | 컨테이너가 본체에 닿는 주소 | 서버가 듣는 주소 |
| --- | --- | --- |
| Docker Desktop(맥·윈도우) | `host.docker.internal` | `127.0.0.1` |
| Colima(`--network-address` 켬) | 본체에 있는 가상 기계 망 게이트웨이(예: `bridge100` 주소) | 그 게이트웨이 주소 |
| Colima(망 주소 없음) | `host.lima.internal` | `127.0.0.1` |
| 리눅스 Docker Engine | 브리지 게이트웨이(`docker network inspect`, 보통 `172.17.0.1`), 방화벽 허용 | 그 게이트웨이 주소 |

Colima 사용자 모드 망은 연속 요청 수백 번 뒤에 연결 하나가 2초 넘게 멈춘 적이 있습니다. 사서가 기다렸다 다시 보내지만, 가상 기계 망 주소를 쓰면 멈춤이 없습니다.

그다음 도서관이 그 서버를 쓰게 합니다. 임베더 컨테이너가 만든 벡터를 그대로 두려면 같은 모델 이름을 줍니다.

```bash
bin/install-project --embed-url http://ADDRESS:18080 --embed-model embeddinggemma-2-BF16@6ea3e3a PROJECT
```

설치기는 임베더 컨테이너를 지웁니다. `--embed-model`을 주지 않으면 모델 이름이 `embeddinggemma-2`가 되어 모든 문서를 다시 임베딩합니다.

### 계속 돌게 하기

맥(`launchd`): 로그인할 때 띄우려면 `~/Library/LaunchAgents/org.benedict.embedder.plist`에, 로그인 없이 부팅 때 띄우려면 `/Library/LaunchDaemons`에 `UserName` 키를 더해 저장합니다. `launchctl bootstrap gui/$(id -u) 파일`(또는 `sudo launchctl bootstrap system 파일`)로 올립니다. 파일 내용은 [영어판](embedding.md#keeping-it-running)과 같습니다.

리눅스(`systemd`): `/etc/systemd/system/benedict-embedder.service`에 영어판의 단위 파일을 저장하고 `sudo systemctl enable --now benedict-embedder`.

## 다른 곳의 임베딩 서버

`--embed-url`은 평문 HTTP로 `POST /v1/embeddings`에 답하는 서버라면 무엇이든 받습니다. 모든 문서와 질문의 글이 그 서버로 가므로 믿을 수 있는 망 안에 두십시오. `--embed-model`로 모델 이름을 주면, 같은 서버·같은 모델 이름을 쓰는 도서관끼리 같은 방식으로 만든 벡터를 함께 쓸 수 있습니다.

## 실패할 때

의미 검색은 도서관을 멈추지 않습니다. 서버가 답하지 않으면 검색은 낱말 결과를 돌려주며 `semantic.status`에 `failed`와 이유을 적고 종료 코드 1로 끝납니다. 사서의 `vectors` 단계도 `failed`를 보고하고 다음 주기에 다시 합니다. 느린 서버는 그 서버의 기록만큼 기다립니다([portability.md](portability.md)의 `benedict_embed_pace`).

