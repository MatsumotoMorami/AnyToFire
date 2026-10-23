const fs = require('node:fs');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const source = fs.readFileSync('src/app.js','utf8');
const prefix = source.slice(0,source.indexOf('const selection ='));
vm.runInNewContext(prefix + `
const scores=[{eventId:'a',exScore:90},{eventId:'b',exScore:1000},{eventId:'c',exScore:null}];
assert.deepEqual(JSON.parse(JSON.stringify([...scores].sort((a,b)=>compareRows(a,b,{key:'exScore',direction:1})).map(x=>x.eventId))),['a','b','c']);
assert.deepEqual(JSON.parse(JSON.stringify([...scores].sort((a,b)=>compareRows(a,b,{key:'exScore',direction:-1})).map(x=>x.eventId))),['b','a','c']);
assert(compareRows({id:'a',occurredAt:'2026-10-23T01:00:00+08:00'},{id:'b',occurredAt:'2026-10-22T18:00:00Z'},{key:'occurredAt',direction:1})<0);
assert(sortHeader('分数','exScore','recent',{key:'exScore',direction:-1}).includes('aria-sort="descending"'));
assert(compareRows({id:'a',name:'档案2'},{id:'b',name:'档案10'},{key:'name',direction:1})<0);
`, {assert});
const demo=fs.readFileSync('ui-demo/index.html','utf8');
for(const match of demo.matchAll(/<script[^>]*>([\s\S]*?)<\/script>/g)) new vm.Script(match[1]);
console.log('PASS numeric/text/timezone/null ordering, sort indicators, demo JS syntax');
