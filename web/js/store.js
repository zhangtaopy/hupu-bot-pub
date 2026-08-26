/**
 * Shared reactive state for the app.
 * Action functions live in their respective component modules.
 */
const { ref, reactive, computed } = Vue;

export function createStore() {
  // ── Dark mode ──
  const darkMode = ref(false);

  function applyDarkMode(val) {
    darkMode.value = val;
    document.documentElement.classList.toggle('dark', val);
    localStorage.setItem('hupu-dark-mode', val ? '1' : '0');
  }

  function toggleDarkMode() {
    applyDarkMode(!darkMode.value);
  }

  const savedDark = localStorage.getItem('hupu-dark-mode');
  if (savedDark === '1') {
    applyDarkMode(true);
  } else if (savedDark === null && window.matchMedia('(prefers-color-scheme: dark)').matches) {
    applyDarkMode(true);
  }

  // ── Tab & search ──
  const activeTab = ref('replies');
  const euid = ref('');
  const threshold = ref(0.5);
  const loading = ref(false);
  const error = ref('');

  // ── Config overlay state ──
  const needsConfig = ref(false);
  const needsAiKey = ref(false);
  const configSaving = ref(false);
  const configError = ref('');
  const configCookie = ref('');
  const configProvider = ref('deepseek');
  const configAiKey = ref('');
  const configModel = ref('');
  const providerModels = { deepseek: 'deepseek-v4-flash', ollama: 'gpt-oss:120b', openrouter: 'google/gemini-2.0-flash-001', opencode: 'opencode/deepseek-v4-flash-free' };
  const providerLabel = computed(() => ({ deepseek: 'DeepSeek', ollama: 'Ollama', openrouter: 'OpenRouter', opencode: 'OpenCode' }[configProvider.value] || 'AI'));
  const providerKeyPlaceholder = computed(() => ({ deepseek: 'sk-...', ollama: 'ollama-api-key...', openrouter: 'sk-or-...', opencode: 'oc-...' }[configProvider.value] || ''));
  const providerModelPlaceholder = computed(() => providerModels[configProvider.value] || '');

  // ── User credentials (browser-local) ──
  const USER_KEY_LS = 'hupu_user_ai_key';
  const USER_PROV_LS = 'hupu_user_ai_provider';
  const userApiProvider = ref(localStorage.getItem(USER_PROV_LS) || 'deepseek');
  const userApiKey = ref(localStorage.getItem(USER_KEY_LS) || '');
  const showUserApiPanel = ref(false);

  function saveUserApiKey() {
    if (userApiKey.value.trim()) {
      localStorage.setItem(USER_KEY_LS, userApiKey.value.trim());
      localStorage.setItem(USER_PROV_LS, userApiProvider.value);
    } else {
      localStorage.removeItem(USER_KEY_LS);
      localStorage.removeItem(USER_PROV_LS);
    }
  }

  function hasUserApiKey() { return !!userApiKey.value.trim(); }

  const userApiKeyPlaceholder = computed(() =>
    ({ deepseek: 'sk-...', ollama: 'ollama-api-key...', openrouter: 'sk-or-...', opencode: 'oc-...' }[userApiProvider.value] || ''));

  function userApiKeyParams() {
    if (!hasUserApiKey()) return '';
    return '&api_key=' + encodeURIComponent(userApiKey.value.trim()) + '&provider=' + encodeURIComponent(userApiProvider.value);
  }

  const USER_COOKIE_LS = 'hupu_user_cookie';
  const userCookie = ref(localStorage.getItem(USER_COOKIE_LS) || '');

  function hasUserCookie() { return !!userCookie.value.trim(); }

  function saveUserCookie() {
    if (userCookie.value.trim()) {
      localStorage.setItem(USER_COOKIE_LS, userCookie.value.trim());
    } else {
      localStorage.removeItem(USER_COOKIE_LS);
    }
  }

  function userCookieParams() {
    if (!hasUserCookie()) return '';
    return '&cookie=' + encodeURIComponent(userCookie.value.trim());
  }

  function hasLocalCredentials() { return hasUserCookie() && hasUserApiKey(); }

  // ── Analysis results ──
  const userInfo = ref(null);
  const stats = ref(null);
  const groups = ref([]);
  const wordCloudWords = ref([]);
  const detailedAnalysis = ref(null);
  const expandedGroups = reactive({});
  const progressPhase = ref('');
  const progressCurrent = ref(0);
  const progressTotal = ref(0);
  const aiLoading = ref(false);
  const aiResult = ref(null);
  const aiProgressPhase = ref('');
  const aiProgressCurrent = ref(0);
  const aiProgressTotal = ref(0);
  const hasSimilarityResults = ref(false);
  const similarityLoading = ref(false);
  const displayedEuid = ref('');

  // ── Euid dropdown ──
  const showEuidDropdown = ref(false);
  const euidsList = ref([]);

  const filteredEuids = computed(() => {
    const q = euid.value.trim().toLowerCase();
    if (!q) return euidsList.value;
    return euidsList.value.filter(item =>
      item.euid.toLowerCase().includes(q) || item.username.toLowerCase().includes(q)
    );
  });

  // ── Fetch data state ──
  const fetchRepliesCount = ref(0);
  const fetchPostsPages = ref(0);
  const fetchIncremental = ref(true); // 增量模式：只抓取尚未入库的新数据，遇到已存在页自动停止
  const fetchLoading = ref(false);
  const fetchResult = ref(null);
  const fetchPostsProgressPhase = ref('');
  const fetchPostsProgressCurrent = ref(0);
  const fetchPostsProgressTotal = ref(0);
  const fetchPostsLoading = ref(false);
  const fetchRepliesProgressPhase = ref('');
  const fetchRepliesProgressCurrent = ref(0);
  const fetchRepliesProgressTotal = ref(0);
  const fetchRepliesLoading = ref(false);

  // ── Posts state ──
  const postsData = ref([]);
  const postLoading = ref(false);
  const aiPostLoading = ref(false);
  const aiPostResult = ref(null);
  const aiPostProgressPhase = ref('');
  const aiPostProgressCurrent = ref(0);
  const aiPostProgressTotal = ref(0);
  const totalReplies = ref(0);
  const totalVisits = ref(0);
  const totalLights = ref(0);
  const videoCount = ref(0);

  // ── QA state ──
  const qaQuestion = ref('');
  const qaHistory = ref([]);
  const qaLoading = ref(false);
  const qaError = ref('');
  const qaUsername = ref('');
  const qaReplyCount = ref(0);
  const qaPostCount = ref(0);
  // QA tab 内的子 tab：ask = 问答，ghost = 魂穿
  const qaSubTab = ref('ask');
  const suggestedQuestions = [
    '这个用户主要在哪些板块活动？',
    '分析一下这个用户的发帖风格和特点',
    '这个用户最关注什么话题？',
    '这个用户在虎扑上最活跃的时期是什么时候？',
    '根据发言推测这个用户的个人信息',
  ];

  // ── Ghost state (成分卡 + 魂穿) ──
  const ghostMode = ref('reply');
  const ghostInput = ref('');
  const ghostHistory = ref([]);
  const ghostLoading = ref(false);
  const ghostError = ref('');
  const ghostUsername = ref('');
  const profileCard = ref(null);
  const profileLoading = ref(false);
  const profileError = ref('');
  const profileCached = ref(false);
  const profileStage = ref('');

  // ── Interaction graph state (互动图谱) ──
  const graphLoading = ref(false);
  const graphError = ref('');
  const graphData = ref(null);       // { main_username, total_interactions, total_targets, shown_targets, nodes, edges }
  const graphDetail = ref(null);     // { name, isMain, total, replies, offset }
  const graphDetailLoading = ref(false);
  const graphDetailMoreLoading = ref(false);

  // ── Computed ──
  const sortedPosts = computed(() =>
    [...postsData.value].sort((a, b) => b.create_time - a.create_time)
  );

  // ── Chart instances (non-reactive) ──
  const chartInstances = { topicChart: null, timeChart: null, hourChart: null, weekdayChart: null, lengthChart: null, postTopicChart: null };
  const timers = { pollTimer: null, aiPollTimer: null, postAiPollTimer: null, debounceTimer: null, fetchPostsPollTimer: null };

  // ── UI helpers ──
  function hasPersonalInfo(pi) {
    if (!pi) return false;
    return !!(pi.age_range || pi.gender || pi.height_weight || pi.relationship
      || pi.hometown || pi.current_city || pi.education || pi.study_abroad
      || pi.profession || pi.income_hint || pi.car || pi.housing
      || pi.personality_traits || pi.political_stance || pi.confidence_note
      || (pi.sports_teams && pi.sports_teams.length)
      || (pi.hobbies && pi.hobbies.length)
      || (pi.games && pi.games.length)
      || (pi.other_clues && pi.other_clues.length));
  }

  function toggleGroup(id) {
    expandedGroups[id] = !expandedGroups[id];
  }

  function selectEuid(selectedEuid) {
    euid.value = selectedEuid;
    showEuidDropdown.value = false;
  }

  function onEuidFocus() { showEuidDropdown.value = true; }
  function onEuidBlur() { setTimeout(() => { showEuidDropdown.value = false; }, 150); }

  function clearDisplayData() {
    userInfo.value = null;
    stats.value = null;
    groups.value = [];
    wordCloudWords.value = [];
    detailedAnalysis.value = null;
    aiResult.value = null;
    aiPostResult.value = null;
    postsData.value = [];
    hasSimilarityResults.value = false;
    displayedEuid.value = '';
  }

  // ── Chart helpers ──
  function chartTextColor() { return darkMode.value ? '#98989f' : '#6e6e73'; }
  function chartGridColor() { return darkMode.value ? 'rgba(255,255,255,0.08)' : 'rgba(0,0,0,0.06)'; }

  return {
    darkMode, applyDarkMode, toggleDarkMode,
    activeTab, euid, threshold, loading, error,
    needsConfig, needsAiKey, configSaving, configError,
    configCookie, configProvider, configAiKey, configModel,
    providerLabel, providerKeyPlaceholder, providerModelPlaceholder,
    userApiProvider, userApiKey, showUserApiPanel, userApiKeyPlaceholder,
    saveUserApiKey, hasUserApiKey, userApiKeyParams,
    userCookie, saveUserCookie, hasUserCookie, userCookieParams, hasLocalCredentials,
    userInfo, stats, groups, wordCloudWords, detailedAnalysis,
    expandedGroups, progressPhase, progressCurrent, progressTotal,
    aiLoading, aiResult, aiProgressPhase, aiProgressCurrent, aiProgressTotal,
    hasSimilarityResults, similarityLoading, displayedEuid,
    showEuidDropdown, euidsList, filteredEuids,
    fetchRepliesCount, fetchPostsPages, fetchIncremental, fetchLoading, fetchResult,
    fetchPostsProgressPhase, fetchPostsProgressCurrent, fetchPostsProgressTotal,
    fetchPostsLoading, fetchRepliesProgressPhase, fetchRepliesProgressCurrent,
    fetchRepliesProgressTotal, fetchRepliesLoading,
    postsData, postLoading, aiPostLoading, aiPostResult,
    aiPostProgressPhase, aiPostProgressCurrent, aiPostProgressTotal,
    totalReplies, totalVisits, totalLights, videoCount,
    qaQuestion, qaHistory, qaLoading, qaError, qaUsername,
    qaReplyCount, qaPostCount, qaSubTab, suggestedQuestions, sortedPosts,
    ghostMode, ghostInput, ghostHistory, ghostLoading, ghostError, ghostUsername,
    profileCard, profileLoading, profileError, profileCached, profileStage,
    graphLoading, graphError, graphData, graphDetail, graphDetailLoading, graphDetailMoreLoading,
    hasPersonalInfo, toggleGroup, selectEuid, onEuidFocus, onEuidBlur, clearDisplayData,
    chartTextColor, chartGridColor,
    chartInstances, timers,
  };
}
