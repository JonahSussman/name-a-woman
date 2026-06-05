/** @typedef {'setup'|'playing'|'completed'|'timed_out'|'results'|'leaderboard'} GameState */
/** @typedef {'women'|'men'|'people'} Category */

const CATEGORIES = ['women', 'men', 'people'];
const COUNTS = [10, 100, 1000];
const CATEGORY_COLORS = { women: '#e91e8a', men: '#1e90ff', people: '#333' };
const INACTIVITY_TIMEOUT = 300_000;

let _chart = null;

function getUserId() {
  let id = localStorage.getItem('naw_user_id');
  if (!id) {
    id = crypto.randomUUID();
    localStorage.setItem('naw_user_id', id);
  }
  document.cookie = `naw_session=${id}; path=/; max-age=31536000; SameSite=Lax`;
  return id;
}

function Game() {
  const userId = getUserId();

  return {
    /** @type {GameState} */
    state: 'setup',
    /** @type {Category} */
    category: 'women',
    targetCount: 100,
    currentGuess: '',
    acceptedGuesses: [],
    acceptedCount: 0,
    gameId: null,
    userId,
    /** @type {WebSocket|null} */
    ws: null,

    startTime: null,
    elapsed: 0,
    inactivityRemaining: INACTIVITY_TIMEOUT,
    lastCorrectTime: null,
    timerFrame: null,
    pausedAt: null,
    pausedElapsed: 0,

    completion: null,

    fallbacksUsed: 0,
    fallbacksMax: 0,

    guessPending: false,
    inputError: false,
    inputWarn: false,
    feedbackMsg: '',
    feedbackClass: '',

    /** @type {object|null} */
    leaderboard: null,
    /** @type {object|null} */
    gameStats: null,
    leaderboardPage: null,
    leaderboardGoTo: '',
    shareMsg: '',

    get categoryColor() {
      return CATEGORY_COLORS[this.category];
    },

    get titleClickable() {
      return this.state === 'setup' || this.state === 'leaderboard';
    },

    shortId(id) {
      return id ? id.slice(0, 8) : '';
    },

    gameUrl(gameId) {
      return `${location.origin}${location.pathname}?game_id=${gameId}`;
    },

    get hasCorrections() {
      return this.acceptedGuesses.some(g => g.corrected);
    },

    get hasFallbacks() {
      return this.fallbacksUsed > 0;
    },

    get paginationPages() {
      const fl = this.leaderboard?.full_leaderboard;
      if (!fl || fl.total_pages <= 1) return [];
      const cur = fl.page;
      const total = fl.total_pages;
      const pages = new Set();
      pages.add(1);
      pages.add(total);
      for (let i = Math.max(2, cur - 1); i <= Math.min(total - 1, cur + 1); i++) {
        pages.add(i);
      }
      const sorted = [...pages].sort((a, b) => a - b);
      const result = [];
      for (let i = 0; i < sorted.length; i++) {
        if (i > 0 && sorted[i] - sorted[i - 1] > 1) {
          result.push(null);
        }
        result.push(sorted[i]);
      }
      return result;
    },

    mounted() {
      const params = new URLSearchParams(window.location.search);
      const gameId = params.get('game_id');
      if (gameId) {
        this.loadSharedResults(gameId);
      }
    },

    async loadSharedResults(gameId) {
      this.state = 'results';
      this.gameId = gameId;
      try {
        const statsRes = await fetch(`/api/stats/game/${gameId}`);
        if (!statsRes.ok) {
          this.state = 'setup';
          return;
        }
        this.gameStats = await statsRes.json();
        this.category = this.gameStats.game.category;
        this.targetCount = this.gameStats.game.target_count;
        this.completion = this.gameStats.ranking;

        const lbRes = await fetch(`/api/leaderboard?category=${this.category}&count=${this.targetCount}&game_id=${gameId}&page=1`);
        if (lbRes.ok) {
          this.leaderboard = await lbRes.json();
        }
        this.$nextTick(() => this.renderChart());
      } catch {
        this.state = 'setup';
      }
    },

    async loadLeaderboard() {
      this.state = 'leaderboard';
      this.leaderboardPage = 1;
      this.leaderboardGoTo = '';
      try {
        const res = await fetch(`/api/leaderboard?category=${this.category}&count=${this.targetCount}&page=1`);
        if (res.ok) {
          this.leaderboard = await res.json();
        }
      } catch {
        // silently fail
      }
    },

    async loadLeaderboardPage(page) {
      if (page < 1) return;
      const fl = this.leaderboard?.full_leaderboard;
      if (fl && page > fl.total_pages) return;
      this.leaderboardPage = page;
      this.leaderboardGoTo = '';
      const gameIdParam = this.gameId ? `&game_id=${this.gameId}` : '';
      try {
        const res = await fetch(`/api/leaderboard?category=${this.category}&count=${this.targetCount}&page=${page}${gameIdParam}`);
        if (res.ok) {
          this.leaderboard = await res.json();
        }
      } catch {
        // silently fail
      }
    },

    goToPage() {
      const page = parseInt(this.leaderboardGoTo, 10);
      if (page && page >= 1) {
        this.loadLeaderboardPage(page);
      }
    },

    cycleCount() {
      if (!this.titleClickable) return;
      const i = COUNTS.indexOf(this.targetCount);
      this.targetCount = COUNTS[(i + 1) % COUNTS.length];
      if (this.state === 'leaderboard') this.loadLeaderboard();
    },

    cycleCategory() {
      if (!this.titleClickable) return;
      const i = CATEGORIES.indexOf(this.category);
      this.category = CATEGORIES[(i + 1) % CATEGORIES.length];
      if (this.state === 'leaderboard') this.loadLeaderboard();
    },

    onInput() {
      if (this.state !== 'setup') return;
      this.state = 'playing';
      this.acceptedGuesses = [];
      this.acceptedCount = 0;
      this.startTime = Date.now();
      this.lastCorrectTime = Date.now();
      this.elapsed = 0;
      this.inactivityRemaining = INACTIVITY_TIMEOUT;
      this.completion = null;
      this.leaderboard = null;
      this.gameStats = null;
      this.clearFeedback();
      this.startTimers();

      const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
      const wsUrl = `${proto}//${location.host}/api/game/ws`;
      this.ws = new WebSocket(wsUrl);

      this.ws.onopen = () => {
        this.ws.send(JSON.stringify({
          action: 'start',
          category: this.category,
          target_count: this.targetCount,
        }));
      };

      this.ws.onmessage = (e) => {
        const msg = JSON.parse(e.data);
        this.handleMessage(msg);
      };

      this.ws.onclose = () => {
        if (this.state === 'playing') {
          this.state = 'timed_out';
          this.stopTimers();
        }
      };

      this.ws.onerror = () => {
        this.showFeedback('Connection error', 'error');
      };
    },

    submitGuess() {
      const name = this.currentGuess.trim();
      if (!name || !this.ws || this.ws.readyState !== WebSocket.OPEN || this.guessPending) return;
      this.guessPending = true;
      this.pausedAt = Date.now();
      this.pausedElapsed = this.elapsed;
      this.showFeedback('Querying...', 'querying');
      this.ws.send(JSON.stringify({ action: 'guess', name }));
    },

    /** @param {object} msg */
    handleMessage(msg) {
      this.guessPending = false;
      if (this.pausedAt) {
        const pauseDuration = Date.now() - this.pausedAt;
        this.startTime += pauseDuration;
        this.lastCorrectTime += pauseDuration;
        this.pausedAt = null;
      }
      this.clearFeedback();
      setTimeout(() => {
        const input = document.querySelector('.input-area input');
        if (input) input.focus();
      });

      switch (msg.type) {
        case 'started':
          this.gameId = msg.game_id;
          this.fallbacksMax = msg.max_fallbacks;
          break;

        case 'accepted': {
          const person = msg.person;
          const url = person.wikipedia_url || person.wikidata_url;
          if (msg.used_fallback) this.fallbacksUsed++;
          this.acceptedGuesses.unshift({
            order: msg.accepted_count,
            displayName: person.display_name,
            url,
            typed: this.currentGuess.trim(),
            corrected: !!msg.corrected_from,
            usedFallback: !!msg.used_fallback,
            timeMs: this.elapsed,
          });
          this.acceptedCount = msg.accepted_count;
          this.currentGuess = '';
          this.lastCorrectTime = Date.now();

          if (msg.game_complete) {
            this.completion = msg.completion;
            this.state = 'completed';
            this.stopTimers();
            this.loadCompletionData();
          }
          break;
        }

        case 'already_guessed':
          this.showFeedback(`Already guessed: ${msg.person.display_name}`, 'warn');
          this.flashWarn();
          break;

        case 'wrong_category':
          this.showFeedback(`${msg.person.display_name} is ${msg.person.gender} in Wikidata, not ${this.category}`, 'warn');
          this.flashWarn();
          break;

        case 'not_found':
          if (msg.fallback_exhausted) {
            this.showFeedback('Not found (no lookups remaining)', 'error');
          } else {
            this.fallbacksUsed++;
            this.showFeedback('Not found', 'error');
          }
          this.flashError();
          break;

        case 'game_timeout':
          this.state = 'timed_out';
          this.stopTimers();
          break;

        case 'error':
          this.showFeedback(msg.message, 'error');
          break;
      }
    },

    async loadCompletionData() {
      if (!this.gameId) return;
      try {
        const [lbRes, statsRes] = await Promise.all([
          fetch(`/api/leaderboard?category=${this.category}&count=${this.targetCount}&game_id=${this.gameId}&page=1`),
          fetch(`/api/stats/game/${this.gameId}`),
        ]);
        if (lbRes.ok) this.leaderboard = await lbRes.json();
        if (statsRes.ok) this.gameStats = await statsRes.json();
        this.$nextTick(() => this.renderChart());
      } catch {
        // silently fail
      }
    },

    renderChart() {
      const canvas = document.getElementById('pace-chart');
      if (!canvas) return;

      const guesses = this.gameStats?.guesses;
      if (!guesses?.length) return;

      if (_chart) {
        _chart.destroy();
        _chart = null;
      }

      const labels = guesses.map(g => g.order);
      const data = guesses.map(g => g.guess_time_ms / 1000);

      _chart = new Chart(canvas, {
        type: 'line',
        data: {
          labels,
          datasets: [{
            label: 'Time (s)',
            data,
            borderColor: CATEGORY_COLORS[this.category] || '#333',
            backgroundColor: 'transparent',
            tension: 0.2,
            pointRadius: guesses.length > 50 ? 0 : 3,
            pointHoverRadius: 4,
          }],
        },
        options: {
          responsive: true,
          maintainAspectRatio: false,
          scales: {
            x: { title: { display: true, text: 'Guess #' } },
            y: { title: { display: true, text: 'Elapsed (s)' }, beginAtZero: true },
          },
          plugins: {
            legend: { display: false },
            tooltip: {
              callbacks: {
                label: (ctx) => {
                  const g = guesses[ctx.dataIndex];
                  return `${g.display_name} — ${this.formatTime(g.guess_time_ms)}`;
                },
              },
            },
          },
        },
      });
    },

    shareResults() {
      if (!this.gameId || !this.completion) return;
      const url = `${location.origin}${location.pathname}?game_id=${this.gameId}`;
      const time = this.formatTime(this.completion.total_time_ms);
      const text = `I was able to name ${this.targetCount} ${this.category} in ${time}! Think you can beat my time?\n${url}`;
      navigator.clipboard.writeText(text).then(() => {
        this.shareMsg = 'Copied!';
        setTimeout(() => { this.shareMsg = ''; }, 2000);
      }).catch(() => {
        this.shareMsg = text;
      });
    },

    startTimers() {
      const tick = () => {
        if (this.state !== 'playing') return;
        if (!this.pausedAt) {
          const now = Date.now();
          if (this.startTime) {
            this.elapsed = now - this.startTime;
          }
          if (this.lastCorrectTime) {
            this.inactivityRemaining = Math.max(0, INACTIVITY_TIMEOUT - (now - this.lastCorrectTime));
          }
        }
        this.timerFrame = requestAnimationFrame(tick);
      };
      this.timerFrame = requestAnimationFrame(tick);
    },

    stopTimers() {
      if (this.timerFrame) {
        cancelAnimationFrame(this.timerFrame);
        this.timerFrame = null;
      }
    },

    /** @param {number} ms */
    formatTime(ms) {
      const totalSeconds = Math.floor(ms / 1000);
      const minutes = Math.floor(totalSeconds / 60);
      const seconds = totalSeconds % 60;
      const centis = Math.floor((ms % 1000) / 10);
      return `${minutes}:${String(seconds).padStart(2, '0')}.${String(centis).padStart(2, '0')}`;
    },

    /** @param {number} ms */
    formatCountdown(ms) {
      const totalSeconds = Math.ceil(ms / 1000);
      const minutes = Math.floor(totalSeconds / 60);
      const seconds = totalSeconds % 60;
      return `${minutes}:${String(seconds).padStart(2, '0')}`;
    },

    flashError() {
      this.inputError = true;
      setTimeout(() => { this.inputError = false; }, 400);
    },

    flashWarn() {
      this.inputWarn = true;
      setTimeout(() => { this.inputWarn = false; }, 400);
    },

    /** @param {string} msg @param {string} cls */
    showFeedback(msg, cls) {
      this.feedbackMsg = msg;
      this.feedbackClass = cls;
    },

    clearFeedback() {
      this.feedbackMsg = '';
      this.feedbackClass = '';
    },

    resetGame() {
      if (this.ws) {
        this.ws.close();
        this.ws = null;
      }
      this.stopTimers();
      if (_chart) {
        _chart.destroy();
        _chart = null;
      }
      window.history.replaceState({}, '', location.pathname);
      this.state = 'setup';
      this.currentGuess = '';
      this.acceptedGuesses = [];
      this.acceptedCount = 0;
      this.gameId = null;
      this.elapsed = 0;
      this.inactivityRemaining = INACTIVITY_TIMEOUT;
      this.startTime = null;
      this.lastCorrectTime = null;
      this.completion = null;
      this.leaderboard = null;
      this.gameStats = null;
      this.leaderboardPage = null;
      this.leaderboardGoTo = '';
      this.guessPending = false;
      this.fallbacksUsed = 0;
      this.shareMsg = '';
      this.clearFeedback();
    },
  };
}
