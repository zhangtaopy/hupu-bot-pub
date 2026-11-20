import test from 'node:test';
import assert from 'node:assert/strict';
import { setupHistoryTab } from '../web/js/components/HistoryTab.js';

function setup() {
  // Minimal synchronous reactive refs exercise request lifecycle without browser dependencies.
  const ref = initial => {
    let value = initial;
    const listeners = [];
    return { listeners, get value() { return value; }, set value(v) { if (v !== value) { value = v; listeners.forEach(f => f()); } } };
  };
  globalThis.Vue = { ref, computed: f => ({ get value() { return f(); } }), watch: (refs, f) => refs.forEach(r => r.listeners.push(f)), onUnmounted() {} };
  const store = { euid: ref('one'), userApiKey: ref(''), userApiProvider: ref('deepseek') };
  const tab = setupHistoryTab(store);
  tab.historyTopic.value = '汽车';
  return { store, tab };
}
const response = report => ({ ok: true, json: async () => ({ coverage: { matched: 1 }, report }) });

test('late response from previous user cannot overwrite current user report', async () => {
  const {store,tab} = setup();
  const queue = [];
  globalThis.fetch = (_url,options) => new Promise(resolve => queue.push({resolve, options}));
  const old = tab.historyLoad(false);
  store.euid.value = 'two';
  assert.equal(queue[0].options.signal.aborted, true);
  const current = tab.historyLoad(false);
  const report = { euid: 'two', analysis: { comparisons: [] } };
  queue[1].resolve(response(report)); await current;
  queue[0].resolve(response({ euid: 'one', analysis: { comparisons: [] } })); await old;
  assert.equal(tab.historyData.value.report.euid, 'two');
  assert.equal(tab.historyLoading.value, false);
});

test('generation failure preserves report and credentials stay in POST body', async () => {
  const {store,tab} = setup();
  store.userApiKey.value = 'test-key';
  const original = { coverage: {matched:1}, report: { analysis: {claims: [], comparisons: []} } };
  tab.historyData.value = original;
  globalThis.fetch = async (url, options) => {
    assert.equal(url, '/api/history');
    const body = JSON.parse(options.body);
    assert.equal(body.api_key, 'test-key'); assert.equal(body.refresh, true);
    return {ok:false, json:async()=>({error:'AI 不可用'})};
  };
  await tab.historyLoad(true);
  assert.equal(tab.historyData.value, original);
  assert.equal(tab.historyError.value, 'AI 不可用');
});

test('invalid dates prevent requests; changing a filter clears prior evidence', async () => {
  const {tab} = setup();
  globalThis.fetch = () => { throw new Error('must not request'); };
  tab.historyStart.value = '2026-11-17'; tab.historyEnd.value = '2026-11-16';
  await tab.historyLoad(true);
  assert.match(tab.historyError.value, /开始日期/);
  tab.historyEvidence.value = {id:'old'};
  tab.historyData.value = {report:{}};
  tab.historyTopic.value = '篮球';
  assert.equal(tab.historyEvidence.value, null);
  assert.equal(tab.historyData.value, null);
});

test('curated original comments carry stored light counts and keep legacy counts unknown', () => {
  const {tab} = setup();
  tab.historyData.value = {report:{report_version:2, evidence:[
    {id:'r:1',content:'先说价格。',light_count:42},
    {id:'r:2',content:'外观挺好。',light_count:0},
  ],analysis:{claims:[
    {id:'c1',evidence_id:'r:1',excerpt:'先说价格。',stance:'价格是重点'},
    {id:'c2',evidence_id:'r:2',excerpt:'外观挺好。',stance:'认可外观'},
  ],highlights:[{claim_id:'c1',comment:'直奔主题。'},{claim_id:'missing',comment:'不显示'},{claim_id:'c1',comment:'重复不显示'}]}}};
  assert.equal(tab.historyLegacy.value,false);
  assert.equal(tab.historyHighlights.value.length,1);
  assert.equal(tab.historyHighlights.value[0].source.light_count,42);
  tab.historyShowClaim('c1');
  assert.equal(tab.historyEvidenceParts.value.excerpt,'先说价格。');
  tab.historyData.value = {report:{evidence:[{id:'r:1',content:'先说价格。'}],analysis:{claims:[{id:'c1',evidence_id:'r:1',excerpt:'先说价格。',stance:'价格是重点'}]}}};
  assert.equal(tab.historyLegacy.value,true);
  assert.equal(tab.historyHighlights.value[0].source.light_count,undefined);
});
