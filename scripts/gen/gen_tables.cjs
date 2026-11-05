const fs=require('fs');
const props=['L','Lu','Ll','Nl','Nd','P','Pc','Z','Zs','Cc','C'];
let out='//! Unicode general-category ranges used by `\\p{..}` in the regex engine.\n//! Generated from V8\'s `\\p` tables. Surrogates are omitted: `char` cannot hold them.\n\n';
for(const p of props){const re=new RegExp('^\\p{'+p+'}$','u');let rs=[];let st=-1;
 for(let c=0;c<=0x110000;c++){const inS=c>=0xD800&&c<=0xDFFF;const m=c<0x110000&&!inS&&re.test(String.fromCodePoint(c));
  if(inS&&st>=0&&p==='C'){continue;}
  if(m){if(st<0)st=c}else if(st>=0){rs.push([st,c-1]);st=-1}}
 out+=`pub static ${p.toUpperCase()==='L'?'L':p.toUpperCase()}: &[(u32, u32)] = &[\n`;
 for(let i=0;i<rs.length;i+=6)out+='    '+rs.slice(i,i+6).map(r=>`(0x${r[0].toString(16)}, 0x${r[1].toString(16)})`).join(', ')+',\n';
 out+='];\n\n';}
fs.writeFileSync('regex_tables.rs',out.trimEnd()+'\n');
