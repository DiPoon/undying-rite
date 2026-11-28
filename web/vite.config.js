import { defineConfig } from 'vite';

// UR_POLL=1 이면 파일 감시를 폴링으로 합니다. Windows 쪽 편집기로 고친 파일은 WSL의 일반 감시에 잡히지 않기 때문입니다.
const poll = process.env.UR_POLL === '1';

export default defineConfig({
  server: {
    watch: poll ? { usePolling: true, interval: 300 } : undefined,
    // 개발 서버(npm run dev)에서 WebSocket은 게임 서버로 넘깁니다.
    proxy: { '/ws': { target: 'ws://127.0.0.1:8080', ws: true } },
  },
  build: { target: 'es2022' },
});
