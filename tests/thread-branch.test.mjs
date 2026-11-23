import test from 'node:test';
import assert from 'node:assert/strict';
import { collectBranch } from '../web/js/components/ThreadCharts.js';

test('follows quoted IDs through nested replies without merging names or double-counting cycles', () => {
  const root = { id:'1',author:'甲' };
  const a = { id:'2',author:'乙' };
  const b = { id:'3',author:'乙' };
  const c = { id:'4',author:'丙' };
  const graph = new Map([['1',[a,b]], ['2',[c]], ['4',[root]], ['unknown',[{id:'5',author:'甲'}]]]);
  const rows = collectBranch(root.id,graph);
  assert.deepEqual(rows.map(r=>[r.source.id,r.parent,r.depth]),[['2','1',1],['4','2',2],['3','1',1]]);
  assert.deepEqual(collectBranch(c.id,new Map()),[]);
});

import { pageBands, radialLayout, rankHotComments } from '../web/js/components/ThreadCharts.js';

test('uneven page bands conserve lights and quoted replies, excluding hot-only supplements', () => {
  const floors = Array.from({length:7}, (_,i)=>({page:i+1,lights:i+1,quotes:i%2 ? ['p'] : []}));
  const bands = pageBands({pages:7,floor_count:7,sources:[{},...floors,{page:1,lights:999,quotes:[]}]},3);
  assert.deepEqual(bands.map(b => [b.start,b.end,b.lights,b.quotes]),[[1,2,3,1],[3,4,7,1],[5,7,18,1]]);
  assert.equal(bands[2].top,floors[6]);
});

test('radial view and ranking preserve nested, overlapping branches and a lone hot comment', () => {
  const [a,b,c,d,e]=['a','b','c','d','e'].map(id=>({id}));
  const children=new Map([['a',[b,c]],['b',[d]]]);
  const ranked=rankHotComments([b,e,a],children);
  assert.deepEqual(ranked.map(r=>[r.source.id,r.direct,r.total]),[['a',2,3],['b',1,1],['e',0,0]]);
  const view=radialLayout(a,children);
  assert.deepEqual(view.edges,[{source:'a',target:'b'},{source:'b',target:'d'},{source:'a',target:'c'}]);
  assert.equal(view.maxDepth,2);
  assert.ok(view.nodes.every(n=>Number.isFinite(n.x) && Number.isFinite(n.y)));
  assert.equal(radialLayout(e,children).edges.length,0);
});
