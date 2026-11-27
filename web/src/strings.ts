// 화면에 나오는 문자열은 전부 여기에 둡니다. 게임 이름과 테마가 정해지면 이 파일만 고칩니다.

export const S = {
  pageTitle: '이름 미정',
  title: '이름 미정 — M0 네트워크 시험',
  roomLabel: '방 코드',
  nameLabel: '이름',
  joinButton: '입장',
  help: '이동: WASD 또는 방향키',
  connecting: '연결 중…',
  disconnected: '서버와 연결이 끊어졌습니다. 새로고침하면 다시 들어갈 수 있습니다.',
  loadFailed: 'WebAssembly를 불러오지 못했습니다.',

  joinError: {
    bad_room_code: '방 코드는 영문자와 숫자 1~12자입니다.',
    bad_name: '이름은 앞뒤 공백을 뺀 1~16자입니다.',
  } as Record<string, string>,

  reject: {
    protocol_mismatch: '서버와 이 페이지의 버전이 다릅니다. 새로고침해 주세요.',
    room_full: '방이 꽉 찼습니다.',
    bad_room_code: '방 코드는 영문자와 숫자 1~12자입니다.',
    bad_name: '이름은 앞뒤 공백을 뺀 1~16자입니다.',
    not_joined: '입장 메시지가 올바르지 않습니다.',
    server_error: '서버가 방을 만들지 못했습니다. 서버 로그를 확인해 주세요.',
  } as Record<string, string>,
  rejectUnknown: '입장이 거절되었습니다.',

  lagNotice: (lag: string, jitter: string) => `지연 주입: 왕복 ${lag}ms ± ${jitter}ms`,

  debug: {
    room: '방',
    rtt: '왕복 지연',
    renderDelay: '상대 그리기 지연',
    pending: '확인 대기 입력',
    lastError: '최근 예측 오차',
    recentMaxError: '5초 최대 예측 오차',
    visualOffset: '남은 화면 보정',
    snaps: '즉시 맞춤 횟수',
    starved: '보간 멈춤 (전체 / 최근)',
    spread: '상태 도착 흔들림',
    margin: '보간 여유',
    tickRate: '서버 틱 속도 (이 시계 기준)',
    perSecond: '/초',
    fps: 'FPS',
    ms: 'ms',
    cm: 'cm',
    none: '-',
  },
};
