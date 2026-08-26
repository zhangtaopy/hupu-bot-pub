/**
 * Data fetching section — batch fetch replies + posts from Hupu API.
 */
import * as api from '../utils/api.js';

export function setupFetchSection(store) {
  let fetchPostsPollTimer = null;

  async function fetchData() {
    if (!store.euid.value.trim()) return;

    if (store.displayedEuid.value && store.displayedEuid.value !== store.euid.value) {
      store.clearDisplayData();
    }

    store.fetchLoading.value = true;
    store.fetchResult.value = null;
    store.error.value = '';

    const maxPages = store.fetchRepliesCount.value === 0 ? 0 : Math.floor(store.fetchRepliesCount.value / 10);
    const pageSize = 10;

    store.fetchPostsLoading.value = true;
    store.fetchPostsProgressPhase.value = '开始获取发帖';
    store.fetchPostsProgressCurrent.value = 0;
    store.fetchPostsProgressTotal.value = 0;

    store.fetchRepliesLoading.value = true;
    store.fetchRepliesProgressPhase.value = '开始获取回帖';
    store.fetchRepliesProgressCurrent.value = 0;
    store.fetchRepliesProgressTotal.value = 0;

    let postsDone = false;
    let postsError = null;

    // Start posts fetch (fire and forget; poll progress separately)
    api.startFetchPosts(
      store.euid.value,
      store.fetchPostsPages.value,
      store.userCookieParams(),
      store.fetchIncremental.value
    ).then(data => {
      if (data.status === 'error') postsError = data.error || '获取发帖失败';
    }).catch(e => {
      postsError = e.message || '获取发帖网络错误';
    });

    // Poll posts progress
    function pollPostsProgress() {
      if (postsDone) return;
      api.fetchPostsProgress(store.euid.value).then(p => {
        if (p.phase && p.phase !== 'idle') store.fetchPostsProgressPhase.value = p.phase;
        if (p.total > 0) {
          store.fetchPostsProgressCurrent.value = p.current;
          store.fetchPostsProgressTotal.value = p.total;
        }
        if (p.done) {
          postsDone = true;
          store.fetchPostsLoading.value = false;
          if (p.error) postsError = p.error;
        } else {
          fetchPostsPollTimer = setTimeout(pollPostsProgress, 1000);
        }
      }).catch(() => {
        fetchPostsPollTimer = setTimeout(pollPostsProgress, 2000);
      });
    }
    pollPostsProgress();

    try {
      const repliesStartData = await api.startFetchReplies(
        store.euid.value,
        maxPages,
        pageSize,
        store.userCookieParams(),
        store.fetchIncremental.value
      );

      if (repliesStartData.status === 'error') {
        throw new Error(repliesStartData.error || '获取回帖失败');
      }

      // Poll replies progress
      async function pollRepliesProgress() {
        const p = await api.fetchRepliesProgress(store.euid.value);
        if (p.phase && p.phase !== 'idle') store.fetchRepliesProgressPhase.value = p.phase;
        if (p.total > 0) {
          store.fetchRepliesProgressCurrent.value = p.current;
          store.fetchRepliesProgressTotal.value = p.total;
        }
        if (p.done) {
          store.fetchRepliesLoading.value = false;
          if (p.error) throw new Error(p.error);
          return p.current;
        }
        await new Promise(r => setTimeout(r, 1000));
        return pollRepliesProgress();
      }

      const repliesFetched = await pollRepliesProgress().catch(e => { throw e; });

      // Wait for posts
      while (!postsDone) {
        await new Promise(r => setTimeout(r, 500));
      }

      if (postsError) throw new Error(postsError);

      store.fetchResult.value = {
        success: true,
        message: `获取成功! 回帖: ${repliesFetched} 条, 发帖: 已完成`,
      };
    } catch (e) {
      store.fetchResult.value = { success: false, message: '网络错误: ' + (e.message || '未知错误') };
      store.error.value = '获取数据失败: ' + (e.message || '网络错误');
    } finally {
      store.fetchLoading.value = false;
      store.fetchRepliesLoading.value = false;
      store.fetchPostsLoading.value = false;
      setTimeout(() => { store.fetchResult.value = null; }, 8000);
    }
  }

  return { fetchData };
}
