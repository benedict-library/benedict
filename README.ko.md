# 베네딕트 도서관 · Benedict

**에이전트와 페르소나를 위한 나의 도서관을 만듭니다.**

[English](README.md) · [도서관 운영 방법](docs/method.ko.md) · [에이전트 사용법](docs/agent-usage.md)

일은 한 세션 안에 끝나지 않습니다. 어제의 에이전트가 남긴 파일을 열어도, 왜 그 방법을 택했는지, 어디까지 확인했는지, 무엇을 결정하지 못했는지는 알기 어렵습니다. 그 사정이 지난 대화에만 남으면 새 에이전트에게 일을 넘길 때마다 사람이 다시 설명해야 합니다.

여러 세션을 거치면 또 다른 문제가 생깁니다. 위키에 적은 지식과 에이전트별 폴더에 둔 스킬은 둘 다 마크다운인데 따로 쌓입니다. 오늘 위키에서 오류를 바로잡아도 내일 다른 에이전트가 펼칠 스킬에는 어제의 절차가 남습니다. 기록이 쌓일수록 어떤 경험을 다음 일에도 적용할 수 있는지 대조하고 정리할 사람이 필요해집니다.

**베네딕트는 누구나 자기 자료로 세울 수 있는 에이전트와 페르소나의 도서관입니다.** 여러 동료가 일하며 얻은 지식과 일하는 방법이 도서관을 세운 사람에게 남습니다. 모델을 바꾸더라도 새 동료는 앞선 결정을 찾아 그 근거를 검토하고, 아직 풀리지 않은 질문부터 이어갈 수 있습니다.

![수도원 도서관에서 두 에이전트가 펼친 책을 건넵니다.](docs/benedict-library.jpg)

## 왜 도서관인가

도서관에는 원본을 남기는 사람, 서로 다른 기록을 대조하는 사서, 훗날 새로운 질문을 가지고 찾아올 독자가 있습니다. 세 사람이 한자리에 모이지 않아도 지식은 그 사이에서 자랍니다. 사서는 한 번의 작업에서 내린 판단을 서류철에 남기고, 여러 기록에서 거듭 확인한 내용은 백과사전에 모읍니다. 반복해서 쓸 수 있는 방법은 근거를 붙여 스킬로 정리합니다. 다음 독자는 어느 단계에서든 서가의 원본으로 돌아가 앞선 판단을 다시 살펴볼 수 있습니다.

베네딕트라는 이름은 누르시아의 베네딕토에게서 빌렸습니다. 그의 『수도규칙』은 중요한 일을 논의할 때 젊은 수도자의 의견까지 듣고, 정해진 시간에는 각자 책을 읽도록 했습니다. 이 전통을 이은 장크트갈렌 수도원에서는 8세기부터 필사한 책이 도서관에 쌓였고, 후대의 학자들은 그 책을 읽고 새로운 저술을 남겼습니다. 베네딕트에서도 새로 온 에이전트와 페르소나는 앞선 동료와 같은 원본을 열 수 있습니다. 앞선 판단에 동의하지 않으면 어떤 대목을 다르게 읽었는지 회의실에 남깁니다. [『수도규칙』 3장](https://pressbooks.palni.org/ruleofbenedict/chapter/rb-03/) · [48장](https://pressbooks.palni.org/ruleofbenedict/chapter/rb-48/) · [장크트갈렌 수도원 도서관의 역사](https://www.stiftsbezirk.ch/en/stiftsbibliothek/history)

## 도서관 구조

![베네딕트 구조도. 서가의 원본을 네 실에서 이용하고, 각 실의 작업은 메모리와 데이터베이스에 기록합니다. 사서가 위키를 정리하고 전령이 외부 작업과 결과를 전달합니다.](docs/architecture.ko.svg)

서가의 원본은 읽기 전용으로 연결합니다. 네 실의 작업 기록은 메모리와 데이터베이스에, 사서가 정리한 지식은 위키에 남깁니다. 네 실 어디서든 **외부 작업 파견**(`room dispatch`)을 요청할 수 있습니다. 전령이 호스트에서 지정한 작업을 실행하면, 다음 세션에서도 요청 번호로 같은 실의 **회신**(`room responses`)을 읽을 수 있습니다. [전령 사용법](docs/courier.md)

## 에이전트는 여기서 어떻게 일하나요?

1. **먼저 했던 일을 찾습니다.** 검색실에서 맡은 일과 관련된 결정과 스킬을 함께 찾고, 열람실에서 원본을 읽습니다. 그때의 조건이 지금도 맞는지 확인합니다.
2. **같은 근거를 놓고 함께 판단합니다.** 회의실에 질문과 원본을 한 번 올립니다. 에이전트와 페르소나는 각자 무엇을 보았고 왜 그렇게 판단했는지 남깁니다. 의견이 갈리면 다음 동료가 양쪽의 근거를 모두 읽을 수 있습니다.
3. **다음 사람이 시작할 자리를 남깁니다.** 결정과 이유, 원본의 위치, 남은 질문을 서류철에 기록합니다. 사서는 새 근거가 들어오면 어떤 설명과 절차를 고쳐야 하는지 살피고 색인을 갱신합니다. 사서실에서 여러 기록을 대조한 뒤 종합한 내용과 출처를 기록합니다. 그 기록을 `doseogwan file --id 기록번호 --as concept`로 백과사전에 올리면 다음 동료가 근거까지 찾아갈 수 있습니다.

개인의 중앙도서관에서 시작해 프로젝트나 전문 분야마다 도서관을 따로 세울 수 있습니다. 연결할 때는 누가 어느 도서관을 열람할 수 있는지 정합니다. 자료 이름까지 비공개여야 하는 도서관은 저장 공간과 운영체제 접근 권한도 분리합니다. [도서관 운영 방법](docs/method.ko.md) · [보안 안내](SECURITY.md)

## 내 프로젝트에 설치하기

에이전트에게 “이 프로젝트에 베네딕트를 설치해 줘”라고 요청하세요. 연결할 폴더와 추가할 프로젝트 지침을 확인하고 승인하면 설치합니다.

**터미널에서 직접 설치하기.** 아래 표에서 운영체제에 맞게 준비한 뒤, 베네딕트 저장소 폴더에서 배시(Bash)로 공통 명령을 실행하세요. 설치 후에도 베네딕트 저장소 폴더를 같은 위치에 두세요. 프로젝트 실행기가 그 안의 도커 컴포즈 설정을 사용합니다.

| 운영체제 | 터미널과 준비 도구 | 기존 프로젝트 경로 예시 |
|---|---|---|
| 윈도우 | 윈도우용 리눅스 하위 시스템(WSL 2)의 우분투 같은 배포판에서 배시를 여세요. [도커 데스크톱의 WSL 연동](https://docs.docker.com/desktop/features/wsl/)에서 해당 배포판을 켜고, 그 안에 jq를 설치하세요. | `/home/alex/projects/notes` |
| 맥 | 도커 데스크톱을 실행하고 jq를 설치한 뒤, 터미널에서 `bash`를 입력하세요. | `/Users/alex/projects/notes` |
| 리눅스 | 도커 엔진, 컴포즈 플러그인, jq를 설치하고 배시를 여세요. 현재 계정이 실행 중인 도커 엔진에 접근할 수 있어야 합니다. | `/home/alex/projects/notes` |

먼저 같은 터미널에서 준비 도구를 확인하세요.

```bash
docker info >/dev/null && docker compose version && jq --version
```

버전이 표시되면 첫 줄을 기존 프로젝트의 실제 절대 경로로 바꾸고 설치와 연결 확인을 진행하세요.

```bash
project_dir="/absolute/path/to/your/project"
bin/install-project "$project_dir"
"$project_dir/.benedict/library" room ping
```

설치 중에 기존 위키, 예제 자료, 빈 도서관 중 하나를 선택하세요. 연결에 성공하면 결과에 `"ready":true`가 표시됩니다. 설치한 프로젝트에서 새 에이전트 세션을 열면 그 도서관을 사용합니다. 자동 설치나 기존 서가 연결 방법은 [설치 안내](docs/docker-setup.md)에 있습니다.

## 예제를 직접 실행하기

저장소를 받은 뒤 에이전트에게 이렇게 부탁하세요.

> 에이전트 사용법을 읽고 예제 도서관을 만들어 줘. 반품 규정을 찾아 원문을 읽고, 다음 세션에서도 근거를 확인할 수 있게 질문과 답을 남겨 줘.

저장소 루트에서 Docker Compose로 예제 도서관을 엽니다. 각 명령은 같은 볼륨을 쓰는 새 컨테이너에서 Rust 프로그램 `doseogwan`을 실행합니다. [에이전트 설치 안내](docs/docker-setup.md)

```sh
docker compose -p benedict-demo build library
docker compose -p benedict-demo run --rm -T -e LIBRARY_START=example library setup --activate
library() { docker compose -p benedict-demo run --rm -T library "$@"; }
library read entities/refund.md
library save --refs entities/refund.md "What is the return window?" "Unused products may be returned within 14 days under the sample policy."
library search "return window"
```

마지막 검색에서 저장한 답을 열면 예제 규정까지 따라갈 수 있습니다.

<details>
<summary>기존 Python 호환 명령</summary>

```bash
python3 bin/library.py init --config /tmp/library-demo/library.json --demo
library() { python3 bin/library.py "$1" --config /tmp/library-demo/library.json "${@:2}"; }
library read --library main entities/refund.md
library save --library main "When can an unused product be returned?" \
  "The sample refund policy allows unused products to be returned within 14 days. For a used product, the cited page does not specify a return window." \
  --refs entities/refund.md
library search "refund"
```

마지막 검색 결과에는 예제 규정과 방금 남긴 답이 함께 나옵니다. 새 세션에는 등록부 경로와 “반품 기한이 며칠인지 근거를 찾아줘”라는 일만 건네 보세요. 저장한 답에서 규정 원문까지 찾아가면 인계가 된 것입니다.

</details>

[설치 환경과 저장소](docs/portability.md) · [실별 명령과 예제](docs/agent-usage.md) · [전령 안내(영문)](docs/courier.md) · [광장 안내(영문)](docs/plaza.md) · [검증 방법](docs/verification.md) · [보안](SECURITY.md) · [기여](CONTRIBUTING.md) · [MIT 라이선스](LICENSE)
