const fs=require('fs');
const base=new Intl.Collator('en',{sensitivity:'base'}), acc=new Intl.Collator('en',{sensitivity:'accent'});
const N=0x180;
const ch=c=>String.fromCodePoint(c);
const nfkd=s=>s.normalize('NFKD');
const isMark=c=>c>=0x300&&c<=0x36f;
const strip=s=>[...s].filter(x=>!isMark(x.codePointAt(0))).join('');
const isLetter=c=>/\p{L}/u.test(ch(c));
const ign=c=>base.compare(ch(c),'')===0;
// how each char is modelled
const kind=new Map(); // c -> {type:'atom'} | {type:'decomp',d,compat} | {type:'exp',x,y}
const az='abcdefghijklmnopqrstuvwxyz';
for(let c=0;c<N;c++){ if(isMark(c))continue; const d=nfkd(ch(c)); const st=strip(d);
  if(c===0x140||c===0x13F){kind.set(c,{type:'one',base:'l'});continue}
  if(d!==ch(c)&&st&&[...d].length===1&&acc.compare(ch(c).toLowerCase(),st.toLowerCase())!==0&&base.compare(ch(c),st)===0&&isLetter(c)&&ch(c).toLowerCase()===ch(c)){kind.set(c,{type:'one',base:st});continue}
  if(d!==ch(c)&&st&&base.compare(ch(c),st)===0){kind.set(c,{type:'decomp',d,compat:d!==ch(c).normalize('NFD')});continue}
  if(c>=0x80&&isLetter(c)&&!ign(c)){let done=false;for(const x of az){for(const y of az){if(base.compare(ch(c),x+y)===0){kind.set(c,{type:'exp',x,y});done=true;break}}if(done)break}if(done)continue}
  kind.set(c,{type:'atom'})}
const extras=[0x2044,0x2BC,0x3BC,0x2C8,0x2D8,0x2D9,0x2DA,0x2DB,0x2DC,0x2DD];
const atoms=[...kind.entries()].filter(([c,k])=>k.type==='atom').map(([c])=>c).concat(extras);
const prim=atoms.filter(c=>!ign(c)).sort((a,b)=>base.compare(ch(a),ch(b))||a-b);
const rank=new Map();const rep=new Map();let r=0,prev=null;
for(const c of prim){if(prev===null||base.compare(ch(prev),ch(c))!==0){r++;prev=c}rank.set(c,2*r);if(!rep.has(2*r))rep.set(2*r,c)}
// marks ordered by secondary weight
const marks=[];for(let c=0x300;c<0x370;c++){if(base.compare(ch(c),'')===0&&acc.compare(ch(c),'')!==0&&ch(c).normalize('NFD').length===1)marks.push(c)}
marks.sort((a,b)=>acc.compare('a'+ch(a),'a'+ch(b))||a-b);
// pseudo secondaries: atoms equal to a letter only at the primary level, and expansions
const pseudo=[];
for(const [c,k] of kind){const l=ch(c).toLowerCase();if(l.length!==1)continue;const lc=l.codePointAt(0);
 if(k.type==='atom'){if(ign(c))continue;const rl=ch(rep.get(rank.get(c))).toLowerCase();if(acc.compare(l,rl)!==0&&lc===c)pseudo.push({p:lc,base:rl,rest:''})}
 else if(k.type==='one'&&lc===c){pseudo.push({p:lc,base:k.base,rest:''})}
 else if(k.type==='exp'&&lc===c){if(acc.compare(l,k.x+k.y)!==0)pseudo.push({p:lc,base:k.x,rest:k.y})}}
const entries=marks.map(m=>({m}));
for(const p of pseudo){let i=0;for(;i<entries.length;i++){const e=entries[i];if(e.m===undefined)continue;if(acc.compare(ch(p.p),p.base+ch(e.m)+p.rest)<0)break}entries.splice(i,0,p)}
const secOf=new Map();const pseudoSec=new Map();let s=2,last=null;
const same=(a,b)=>{if(a.m!==undefined&&b.m!==undefined)return acc.compare('a'+ch(a.m),'a'+ch(b.m))===0;if(a.m===undefined&&b.m!==undefined)return acc.compare(ch(a.p),a.base+ch(b.m)+a.rest)===0;if(a.m!==undefined&&b.m===undefined)return acc.compare(ch(b.p),b.base+ch(a.m)+b.rest)===0;return false};
for(const e of entries){if(!last||!same(last,e))s++;if(e.m!==undefined)secOf.set(e.m,s);else pseudoSec.set(e.p,s);last=e}
// canonical combining order ranks, probed against reference marks
const refs=[0x334,0x321,0x31B,0x323,0x300,0x315,0x35C,0x35D,0x345];
const cccRank=m=>{let n=0;for(const x of refs){if(x===m)continue;const t=(ch(x)+ch(m));if(('a'+t).normalize('NFD')!=='a'+t)n++}return refs.length-n};
const lcOf=c=>{const l=ch(c).toLowerCase();return l.length===1?l.codePointAt(0):c};
function atomCes(c,ter){if(ign(c))return[];const out=[[rank.get(c),1,ter]];const l=lcOf(c);if(pseudoSec.has(l))out.push([0,pseudoSec.get(l),1]);return out}
function ces(c,compat){const k=kind.get(c);const U=ch(c).toLowerCase()!==ch(c);
 if(!k||k.type==='atom')return atomCes(c,compat?(U?4:3):(U?2:1));
 if(k.type==='exp'){const sp=pseudoSec.get(lcOf(c));const t=(acc.compare(ch(lcOf(c)),k.x+k.y)===0)?(U?4:3):(U?2:1);const a=[[rank.get(k.x.codePointAt(0)),1,t]];if(sp)a.push([0,sp,1]);a.push([rank.get(k.y.codePointAt(0)),1,t]);return a}
 if(k.type==='one'){return [[rank.get(k.base.codePointAt(0)),1,U?2:1],[0,pseudoSec.get(lcOf(c)),1]]}
 const out=[];for(const xs of k.d){const x=xs.codePointAt(0);if(isMark(x)){if(secOf.has(x))out.push([0,secOf.get(x),1]);continue}
  const sub=(x===c)?atomCes(x,k.compat?(U?4:3):(U?2:1)):ces(x,k.compat);out.push(...sub)}
 return out}
const table=[];for(let c=0;c<N;c++){if(isMark(c)){table.push([]);continue}table.push(ces(c,false))}
const sym=rank.get(0x30)-1;
let out=`// Generated from \`Intl.Collator('en')\` by probing. See \`discovery.rs\`.\n\n`;
out+=`const SYMBOL_PRIMARY: u32 = ${sym};\n\n#[rustfmt::skip]\nstatic LATIN_CES: [&[Ce]; ${N}] = [\n`;
for(const t of table)out+='    &['+t.map(x=>`(${x[0]},${x[1]},${x[2]})`).join(',')+'],\n';
const ms=[],cs=[];for(let c=0x300;c<0x370;c++){let sec=secOf.get(c)||0;const nf=ch(c).normalize('NFD');if(nf.length===1&&sec===0&&false)sec=0;if(nf.length===1){ms.push(sec)}else{const f=nf.codePointAt(0);ms.push(secOf.get(f)||0)}cs.push(cccRank(c))}
out+='];\n\n#[rustfmt::skip]\nstatic MARK_SECONDARY: [u16; 112] = [\n    '+ms.join(',')+',\n];\n';
// combining order per secondary weight, for reordering runs of primary-ignorable CEs
const secCcc=new Array(s+2).fill(6);for(let c=0x300;c<0x370;c++){const v=ms[c-0x300];if(v&&ch(c).normalize('NFD')===ch(c))secCcc[v]=cs[c-0x300]}
out+='\n#[rustfmt::skip]\nstatic SECONDARY_CCC: ['+'u8; '+secCcc.length+'] = [\n    '+secCcc.join(',')+',\n];\n';
fs.writeFileSync(process.argv[2],out);
console.log('marks',marks.length,'pseudo',pseudo.map(p=>ch(p.p)+'='+p.base+p.rest).join(' '));
console.log('exp',[...kind].filter(([c,k])=>k.type==='exp').map(([c,k])=>ch(c)+'='+k.x+k.y).join(' '));
