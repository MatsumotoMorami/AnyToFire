const fs = require('fs');
const crypto = require('crypto');
const path = process.argv[2];
const b = fs.readFileSync(path);
const pe = b.readUInt32LE(0x3c), count = b.readUInt16LE(pe + 6), opt = pe + 24;
const image = b.readBigUInt64LE(opt + 24);
const sections=[];
for(let i=0;i<count;i++){const p=opt+b.readUInt16LE(pe+20)+i*40; sections.push({name:b.toString('ascii',p,p+8).replace(/\0.*/,''),rva:b.readUInt32LE(p+12),size:b.readUInt32LE(p+16),raw:b.readUInt32LE(p+20)});}
function offset(rva){const s=sections.find(s=>rva>=s.rva&&rva<s.rva+s.size); if(!s)throw Error('RVA '+rva.toString(16));return s.raw+rva-s.rva}
function rva(raw){const s=sections.find(s=>raw>=s.raw&&raw<s.raw+s.size);if(!s)throw Error('raw '+raw.toString(16));return s.rva+raw-s.raw}
function ptr(raw){return b.readBigUInt64LE(raw)}
function matches(needle){let out=[],at=0;while((at=b.indexOf(needle,at))>=0){out.push(at);at++}return out}
function named(name){for(const at of matches(Buffer.from(name+'\0'))){let va=image+BigInt(rva(at));const p=Buffer.alloc(8);p.writeBigUInt64LE(va);for(const hit of matches(p)){try{let f=b.readBigUInt64LE(hit+24);if(f>=image&&f<image+0x2000000n)console.log(name,'string',rva(at).toString(16),'slot',rva(hit+24).toString(16),'target',Number(f-image).toString(16),'preceding',b.toString('ascii',at-12,at+name.length+4).replace(/[^\x20-\x7e]/g,'.'));}catch{}}}}
function hash(start,end){let x=b.subarray(offset(start),offset(start)+(end-start));let fnv=y=>{let h=0xcbf29ce484222325n;for(const c of y)h=BigInt.asUintN(64,(h^BigInt(c))*0x100000001b3n);return '0x'+h.toString(16)};let sha=y=>crypto.createHash('sha256').update(y).digest('hex');return {rva:start.toString(16),end:end.toString(16),prefixFnv:fnv(x.subarray(0,32)),tailFnv:fnv(x.subarray(32)),prefixSha:sha(x.subarray(0,32)),tailSha:sha(x.subarray(32))}}
console.log(path,'timestamp',b.readUInt32LE(pe+8).toString(16),'imageSize',b.readUInt32LE(opt+56).toString(16));
for(const name of ['getrank','getranksub','get','reg','appoint'])named(name);
if(process.argv[3]){for(const spec of process.argv.slice(3)){const [a,z]=spec.split('-').map(x=>parseInt(x,16));console.log(JSON.stringify(hash(a,z)))}}
const slots = b.readUInt32LE(pe+8)===0x69c347cc?[0x104b5a8,0x104abe8,0x104b660,0x104b668,0x104b6c8]:b.readUInt32LE(pe+8)===0x69ddf5f8?[0x10535f8,0x1052c38,0x10536b0,0x10536b8,0x1053718]:b.readUInt32LE(pe+8)===0x6a8bafd1?[0x1090d88,0x10903c8,0x1090e40,0x1090e48,0x1090ea8]:[0x10540e8,0x10536c8,0x10541a0,0x10541a8,0x1054208];
for(const slot of slots){const target=Number(ptr(offset(slot))-image);if(target>0&&target<0x2000000)console.log('method',slot.toString(16),target.toString(16),crypto.createHash('sha256').update(b.subarray(offset(target),offset(target)+256)).digest('hex'));}
