const fs=require('fs'), crypto=require('crypto');
if(process.argv.length!==7)throw Error('Usage: node tools/generate-builds.cjs <0331.dll> <0422.dll> <0909.dll> <0916.dll> <0917.dll>');
const fixtures=[
 ['0331',process.argv[2],33,[0x104b5a8,0x104abe8,0x104b660,0x104b668,0x104b6c8]],
 ['0422',process.argv[3],33,[0x10535f8,0x1052c38,0x10536b0,0x10536b8,0x1053718]],
 ['0909',process.argv[4],33,[0x1090d88,0x10903c8,0x1090e40,0x1090e48,0x1090ea8]],
 ['0916',process.argv[5],34,[0x10540e8,0x10536c8,0x10541a0,0x10541a8,0x1054208]],
 ['0917',process.argv[6],34,[0x10540e8,0x10536c8,0x10541a0,0x10541a8,0x1054208]],
];
let out=`// Generated from the five user-supplied bm2dx.dll files by tools/generate-builds.cjs.\n`;
out+=`#[derive(Clone, Copy)]\npub struct MethodSignature { pub rva: usize, pub slot: usize, pub sha256: &'static str }\n`;
out+=`pub struct Build { pub name: &'static str, pub version: u32, pub timestamp: u32, pub image_size: u32, pub rank: MethodSignature, pub profile: MethodSignature, pub reg: MethodSignature, pub reg_response: MethodSignature, pub appoint: MethodSignature }\n`;
out+=`pub const BUILDS: &[Build] = &[\n`;
for(const [name,path,version,slots] of fixtures){
 const b=fs.readFileSync(path),pe=b.readUInt32LE(0x3c),opt=pe+24,base=b.readBigUInt64LE(opt+24),n=b.readUInt16LE(pe+6),sectionStart=opt+b.readUInt16LE(pe+20),secs=[];
 for(let i=0;i<n;i++){const p=sectionStart+i*40;secs.push({rva:b.readUInt32LE(p+12),size:b.readUInt32LE(p+16),raw:b.readUInt32LE(p+20)})}
 const off=rva=>{const s=secs.find(x=>rva>=x.rva&&rva<x.rva+x.size);if(!s)throw Error(name+': '+rva.toString(16));return s.raw+rva-s.rva};
 out+=`    Build { name: "${name}", version: ${version}, timestamp: 0x${b.readUInt32LE(pe+8).toString(16)}, image_size: 0x${b.readUInt32LE(opt+56).toString(16)},\n`;
 for(let i=0;i<5;i++){const slot=slots[i],rva=Number(b.readBigUInt64LE(off(slot))-base),sha=crypto.createHash('sha256').update(b.subarray(off(rva),off(rva)+256)).digest('hex');
 out+=`        ${['rank','profile','reg','reg_response','appoint'][i]}: MethodSignature { rva: 0x${rva.toString(16)}, slot: 0x${slot.toString(16)}, sha256: "${sha}" },\n`}
 out+=`    },\n`;
}
out+=`];\n`;
if(process.env.VERIFY_BUILDS==='1'){
 if(fs.readFileSync('src/builds.rs','utf8')!==out)throw Error('build signatures differ from supplied DLLs');
 console.log('All five build signatures match their supplied DLLs');
}else fs.writeFileSync('src/builds.rs',out);
