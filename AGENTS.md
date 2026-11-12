# 행복한 옥키 작업 지침

이 프로젝트는 [OKKY](https://okky.kr)의 고양이 마스코트를 바탕으로 만든 Windows 데스크톱 펫입니다. 앱 표시 이름은 **행복한 옥키**, 영문 이름은 **Happy Okky**, 기본 펫 이름은 **옥키 (Okky)**입니다. Cargo 패키지와 실행 파일 이름은 `happy-okky`입니다.

## 파일별 역할

| 파일 | 역할 |
| --- | --- |
| `src/main.rs` | 앱 시작, DPI 설정, 중복 실행 처리 |
| `src/native.rs` | Win32 창과 알림 영역 메뉴, 작업 표시줄·모니터 배치, 드래그, 공 던지기, 저장 |
| `src/panel.rs` | egui 상태창, 테마, 이름 편집, 돌보기 입력 |
| `src/game.rs` | 포만감과 기분, 돌보기 쿨타임, 수면 상태, 저장 데이터 읽기·쓰기 형식 |
| `src/behavior.rs`, `src/motion.rs` | 자율 행동 선택과 방향·시간 보간 |
| `src/render.rs`, `src/render/morph.rs` | 스프라이트 로딩, Win32 레이어 그리기, 자세 사이의 프레임 합성 |
| `src/typing.rs` | Windows Raw Input에서 타이핑 속도를 집계 |
| `assets/`, `build.rs` | 그림·음원과 실행 파일용 아이콘 리소스 |
| `tests/windows_lifecycle.rs` | Windows 데스크톱에서 실행·조작·저장·종료를 확인하는 통합 테스트 |

## 변경할 때

- Windows MSVC, Rust, egui, Win32를 사용하는 기존 구조에서 작업합니다. 의존성이나 저장 형식 변경은 해당 기능에 필요한 범위로 제한합니다.
- 상태창은 `Command`를 보내고 데스크톱 스레드가 처리합니다. 화면에 표시할 값은 `Status`를 통해 전달합니다. 창 메시지와 애니메이션을 처리하는 동안 파일 작업이나 대기를 추가할 때 반응 속도를 확인합니다.
- 행동을 추가할 때는 시작 조건, 동작 순서, 끝나는 조건을 정의합니다. 수면·드래그·돌보기처럼 행동을 끊을 수 있는 입력과 맞물리는지도 확인합니다.
- 방향이 있는 동작에는 기존 좌우 15도 자세를 활용합니다. 자세가 크게 바뀌는 구간은 중간 자세를 연결합니다. 새 투명 스프라이트를 추가할 때는 셀 경계, 발 기준선, 좌우 색 무늬, 필요한 입 위치를 확인합니다.
- 기존 저장 위치인 `%LOCALAPPDATA%\NyangArchi\pet.txt`와 이전 저장 형식의 읽기 호환성을 유지합니다. 타이핑 속도 집계는 메모리에서 처리하며 텍스트나 키 기록을 저장하거나 전송하지 않습니다.
- 사용자 조작이나 빌드 방식이 바뀌면 한국어와 영어 README를 함께 갱신합니다. `CLAUDE.md`는 이 파일을 불러옵니다. 작업 결과물 중 소스와 필요한 에셋은 추적하고, `target/` 아래 빌드 및 배포 산출물은 Git에서 제외합니다.

## 검증

저장소 루트의 PowerShell에서 변경 범위에 맞게 실행합니다.

```powershell
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

창 동작, 입력, 저장, 배포 아이콘을 바꿨다면 실제 Windows 데스크톱에서도 확인합니다. 통합 테스트는 별도의 임시 저장 경로를 쓰고 테스트 앱을 직접 실행합니다. 같은 창 클래스를 쓰는 실행 중인 인스턴스를 종료한 뒤 실행하세요.

```powershell
cargo test --release --locked --test windows_lifecycle -- --ignored --nocapture
```

새 애니메이션은 실행 화면에서 양방향 전환이나 중단 후 복귀를 확인합니다. 결과를 보고할 때 실행한 테스트와 환경상 확인하지 못한 항목을 명시합니다. 기능 요청과 검증 예시는 [AI Agent 가이드](docs/AI_AGENT_GUIDE.md)를 참고합니다.
