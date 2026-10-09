import io,re,sys,os,gzip
sys.stdout.reconfigure(encoding='utf-8',errors='replace')
RAW='data/raw'
CJK=re.compile(r'^[\u4e00-\u9fff]+$')
freq=[]
with io.open(os.path.join(RAW,'jieba_dict.txt'),encoding='utf-8') as f:
    for line in f:
        p=line.split()
        if len(p)>=2 and p[1].isdigit() and CJK.match(p[0]): freq.append((p[0],int(p[1])))
freq.sort(key=lambda x:-x[1])
have=set()
with io.open('data/zh-ru.tsv',encoding='utf-8') as f:
    next(f)
    for line in f: have.add(line.split('\t')[0])
# 拼音：从 CC-CEDICT 取
py={}
line_re=re.compile(r'^(\S+)\s+(\S+)\s+\[([^\]]*)\]\s+/(.*)/\s*$')
TONE={'a':'āáǎà','e':'ēéěè','i':'īíǐì','o':'ōóǒò','u':'ūúǔù','ü':'ǖǘǚǜ'}
PLAIN={c:v for v,s in TONE.items() for c in s}
def parse(num):
    out=[]
    for s,d in re.findall(r'([A-Za-zü:ê]+)([1-5])',num or ''):
        s=s.replace('u:','ü').replace('v','ü').lower()
        t=int(d)
        if t!=5 and s:
            for i,ch in enumerate(s):
                if ch in 'aeiouü': pass
            # 简化的声调处理：只做无声调
        out.append(''.join(PLAIN.get(c,c) for c in s))
    return ''.join(out)
with gzip.open(os.path.join(RAW,'cedict.txt.gz'),'rt',encoding='utf-8') as f:
    for line in f:
        if line.startswith('#'): continue
        m=line_re.match(line.rstrip('\n'))
        if not m: continue
        trad,simp,num,defs=m.group(1),m.group(2),m.group(3),m.group(4)
        a=parse(num)
        if simp not in py or (len(defs)>len(py[simp][1])):
            py[simp]=(a,defs)
miss=[(w,fr,py.get(w,('',''))[0],py.get(w,('',''))[1]) for w,fr in freq[:4000] if w not in have]
miss=[m for m in miss if m[2]]
print('前 4000 常用词里缺俄语且能查到拼音的: %d 个' % len(miss))
with io.open('data/raw/_missing.txt','w',encoding='utf-8',newline='\n') as f:
    for w,fr,a,d in miss[:400]:
        f.write('%s\t%s\t%s\n' % (w,a,d[:70]))
print('已写出前 400 个到 data/raw/_missing.txt')
for w,fr,a,d in miss[:25]:
    print('  %s [%s] %s' % (w,a,d[:40]))
