const fs=require('fs'), crypto=require('crypto');
const builds=[
 ['0331',33,[0x104b5a8,0x104abe8,0x104b660,0x104b668,0x104b6c8]],
 ['0422',33,[0x10535f8,0x1052c38,0x10536b0,0x10536b8,0x1053718]],
 ['0909',33,[0x1090d88,0x10903c8,0x1090e40,0x1090e48,0x1090ea8]],
 ['0916',34,[0x10540e8,0x10536c8,0x10541a0,0x10541a8,0x1054208]],
 ['0917',34,[0x10540e8,0x10536c8,0x10541a0,0x10541a8,0x1054208]],
 ['1013',34,[0x10530d8,0x10526b8,0x1053190,0x1053198,0x10531f8]],
 ['1027',34,[0x10250a8,0x1024688,0x1025160,0x1025168,0x10251c8]],
 ['012-0916',34,[0xf550e8,0xf546c8,0xf551a0,0xf551a8,0xf55208]],
 ['012-1027',34,[0xf260a8,0xf25688,0xf26160,0xf26168,0xf261c8]],
];
const args=process.argv.slice(2);
const single=args[0]==='--build' && args.length===3 && builds.find(([name])=>name===args[1]);
if(!single && (args[0]==='--build' || args.length<5 || args.length>builds.length))
 throw Error('Usage: node tools/generate-builds.cjs --build <build> <bm2dx.dll>\n   or: node tools/generate-builds.cjs <0331.dll> <0422.dll> <0909.dll> <0916.dll> <0917.dll> [1013.dll] [1027.dll] [012-0916.dll] [012-1027.dll]');
const fixtures=single?[[single,args[2]]]:args.map((path,i)=>[builds[i],path]);
const existing=fs.readFileSync('src/builds.rs','utf8');
let out=existing;
for(const [[name,version,slots],path] of fixtures){
 const b=fs.readFileSync(path),pe=b.readUInt32LE(0x3c),opt=pe+24,base=b.readBigUInt64LE(opt+24),n=b.readUInt16LE(pe+6),sectionStart=opt+b.readUInt16LE(pe+20),secs=[];
 if(b.readUInt16LE(0)!==0x5a4d || b.readUInt32LE(pe)!==0x4550 || b.readUInt16LE(pe+4)!==0x8664 || b.readUInt16LE(opt)!==0x20b)
  throw Error(name+': expected a Windows x64 PE');
 for(let i=0;i<n;i++){const p=sectionStart+i*40;secs.push({rva:b.readUInt32LE(p+12),size:b.readUInt32LE(p+16),raw:b.readUInt32LE(p+20),executable:(b.readUInt32LE(p+36)&0x20000000)!==0})}
 const off=(rva,len,executable)=>{const s=secs.find(x=>rva>=x.rva&&rva+len<=x.rva+x.size&&x.executable===executable);if(!s || s.raw+rva-s.rva+len>b.length)throw Error(name+': invalid RVA '+rva.toString(16));return s.raw+rva-s.rva};
 let entry='    Build { name: "'+name+'", version: '+version+', timestamp: 0x'+b.readUInt32LE(pe+8).toString(16)+', image_size: 0x'+b.readUInt32LE(opt+56).toString(16)+',\n';
 for(const [i,method] of ['rank','profile','reg','reg_response','appoint'].entries()){
  const slot=slots[i],rva=Number(b.readBigUInt64LE(off(slot,8,false))-base),start=off(rva,256,true);
  const sha=crypto.createHash('sha256').update(b.subarray(start,start+256)).digest('hex');
  entry+='        '+method+': MethodSignature { rva: 0x'+rva.toString(16)+', slot: 0x'+slot.toString(16)+', sha256: "'+sha+'" },\n';
 }
 entry+='    },\n';
 const old=new RegExp('    Build \\{ name: "'+name+'",[\\s\\S]*?\\n    \\},\\n');
 out=old.test(out)?out.replace(old,entry):out.replace(/\];\n$/,entry+'];\n');
}
if(process.env.VERIFY_BUILDS==='1'){
 if(existing!==out)throw Error('build signatures differ from supplied DLLs');
 console.log(fixtures.map(([[name]])=>name).join(', ')+': all five method signatures and dispatch slots match');
}else fs.writeFileSync('src/builds.rs',out);
