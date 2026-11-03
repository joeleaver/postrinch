#!/usr/bin/env python3
# Side-by-side contact sheets: python3 sheet.py <mode> [<mode>...] -> out/sheet_<fixture>.png
import sys,os
from PIL import Image,ImageDraw
modes=sys.argv[1:]
for f in sorted(os.listdir('fixtures')):
    n=f[:-5]; ims=[]
    for m in modes:
        p=f'out/{m}/{n}.png'
        ims.append((m,Image.open(p).convert('RGB') if os.path.exists(p) else Image.new('RGB',(700,1100),'#fcc')))
    H=760; s=Image.new('RGB',(sum(i.width+8 for _,i in ims),H+22),'#444'); x=0; d=ImageDraw.Draw(s)
    for m,i in ims:
        s.paste(i.crop((0,0,i.width,H)),(x,22)); d.text((x+4,5),f'{n} / {m}',fill='white'); x+=i.width+8
    s.save(f'out/sheet_{n}.png')
