# Sound recipes

Every sound in this pack is generated with FFmpeg (Cutline project, CC0-1.0).
Per file: source (lavfi), filter chain, gain to a peak of -2 dBFS, and encoding. Commands per file:

```
ffmpeg -f lavfi -i "<source>" -af "<filter>" -ar 48000 raw.wav
ffmpeg -i raw.wav -af "volume=<gain>dB,alimiter=limit=0.89:level=disabled" -c:a libvorbis -q:a 3 -ar 48000 audio/<id>.ogg
```

`<filter>` always ends with `silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse` (no silence at the start, 25 ms fade-out at the end).

## whoosh-lang (1.5 s, gain 2.2 dB)

```
-f lavfi -i "aevalsrc='(random(0)*2-1)*pow(sin(PI*t/1.5),2)*0.9':s=48000:d=1.5"
-af "highpass=f=250,bandpass=f=1400:width_type=h:w=2400,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## whoosh-links-rechts (1.0 s, gain 6.0 dB)

```
-f lavfi -i "aevalsrc='(random(0)*2-1)*pow(sin(PI*t),2)*(1-t)*0.9|(random(0)*2-1)*pow(sin(PI*t),2)*t*0.9':s=48000:d=1.0"
-af "highpass=f=300,lowpass=f=5000,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## whoosh-rechts-links (1.0 s, gain 6.0 dB)

```
-f lavfi -i "aevalsrc='(random(0)*2-1)*pow(sin(PI*t),2)*t*0.9|(random(0)*2-1)*pow(sin(PI*t),2)*(1-t)*0.9':s=48000:d=1.0"
-af "highpass=f=300,lowpass=f=5000,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## whoosh-tief (1.3 s, gain 8.5 dB)

```
-f lavfi -i "aevalsrc='(random(0)*2-1)*pow(sin(PI*t/1.3),2)*0.9':s=48000:d=1.3"
-af "lowpass=f=700,highpass=f=60,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## swoosh-hoch (0.5 s, gain 0.7 dB)

```
-f lavfi -i "aevalsrc='(random(0)*2-1)*pow(sin(PI*t/0.5),2)*0.7':s=48000:d=0.5"
-af "highpass=f=2500,lowpass=f=9000,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## riser-1s (1 s, gain 1.5 dB)

```
-f lavfi -i "aevalsrc='(0.5*sin(2*PI*(200*t+1400.0*t*t))+0.3*(random(0)*2-1))*pow(t/1,1.8)':s=48000:d=1"
-af "lowpass=f=7000,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## riser-2s (2 s, gain 0.6 dB)

```
-f lavfi -i "aevalsrc='(0.5*sin(2*PI*(200*t+700.0*t*t))+0.3*(random(0)*2-1))*pow(t/2,1.8)':s=48000:d=2"
-af "lowpass=f=7000,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## riser-4s (4 s, gain 0.5 dB)

```
-f lavfi -i "aevalsrc='(0.5*sin(2*PI*(200*t+350.0*t*t))+0.3*(random(0)*2-1))*pow(t/4,1.8)':s=48000:d=4"
-af "lowpass=f=7000,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## downer-1s (1 s, gain 1.5 dB)

```
-f lavfi -i "aevalsrc='(0.6*sin(2*PI*(1600*t-725.0*t*t))+0.15*(random(0)*2-1))*pow(1-t/1,1.5)':s=48000:d=1"
-af "lowpass=f=5000,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## downer-2s (2 s, gain 1.5 dB)

```
-f lavfi -i "aevalsrc='(0.6*sin(2*PI*(1600*t-362.5*t*t))+0.15*(random(0)*2-1))*pow(1-t/2,1.5)':s=48000:d=2"
-af "lowpass=f=5000,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## sub-drop (2.6 s, gain -1.4 dB)

```
-f lavfi -i "aevalsrc='0.95*sin(2*PI*(130*t-22*t*t))*exp(-t*1.1)*min(1,t*400)':s=48000:d=2.6"
-af "lowpass=f=300,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## impact-hart (0.6 s, gain -2.0 dB)

```
-f lavfi -i "aevalsrc='sin(2*PI*(90*t-60*t*t))*exp(-t*9)*0.9+(random(0)*2-1)*exp(-t*60)*0.5':s=48000:d=0.6"
-af "lowpass=f=8000,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## impact-kino (2.4 s, gain -2.0 dB)

```
-f lavfi -i "aevalsrc='sin(2*PI*(55*t-10*t*t))*exp(-t*2.2)*0.9+0.3*sin(2*PI*110*t)*exp(-t*6)+(random(0)*2-1)*exp(-t*20)*0.4':s=48000:d=2.4"
-af "lowpass=f=3000,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## tapestop-lang (1.6 s, gain 1.0 dB)

```
-f lavfi -i "aevalsrc='st(1,2*PI*234.67*(1-pow(1-t/1.6,3)));0.5*(sin(ld(1))+0.5*sin(2*ld(1))+0.3*sin(3*ld(1)))*pow(1-t/1.6,0.5)':s=48000:d=1.6"
-af "lowpass=f=4000,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## ruecklauf (1.0 s, gain 2.6 dB)

```
-f lavfi -i "aevalsrc='(0.5*sin(2*PI*(500*t+6000*mod(t,0.06)))+0.25*(random(0)*2-1))*min(1,t*20)*(1-t*0.7)':s=48000:d=1.0"
-af "highpass=f=300,lowpass=f=7000,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## glitch-digital (0.5 s, gain 3.1 dB)

```
-f lavfi -i "aevalsrc='st(0,200+1500*(sin(floor(t*40)*12.9898)*0.5+0.5));0.5*sgn(sin(2*PI*ld(0)*t))*lt(mod(t,0.05),0.035)':s=48000:d=0.5"
-af "lowpass=f=6000,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## stottern (0.7 s, gain 0.9 dB)

```
-f lavfi -i "aevalsrc='(0.5*sin(2*PI*300*t)+0.3*sin(2*PI*600*t))*lt(mod(t*(1+6*t),0.1),0.05)':s=48000:d=0.7"
-af "lowpass=f=3000,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## blubb (0.15 s, gain -0.9 dB)

```
-f lavfi -i "aevalsrc='sin(2*PI*(300*t+12000*t*t))*exp(-t*35)*min(1,t*400)':s=48000:d=0.15"
-af "silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## plopp-tief (0.2 s, gain -1.9 dB)

```
-f lavfi -i "aevalsrc='sin(2*PI*(150*t+350*0.03*(1-exp(-t/0.03))))*exp(-t*28)':s=48000:d=0.2"
-af "silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## ding-hell (1.2 s, gain 1.2 dB)

```
-f lavfi -i "aevalsrc='0.6*sin(2*PI*1568*t)*exp(-t*5)+0.2*sin(2*PI*3136*t)*exp(-t*9)':s=48000:d=1.2"
-af "silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## glocke (2.5 s, gain 3.4 dB)

```
-f lavfi -i "aevalsrc='0.25*(gte(t,0)*1*exp(-(t-0)*1.60)*sin(2*PI*523*(t-0))+gte(t,0)*0.6*exp(-(t-0)*2.08)*sin(2*PI*1046*(t-0))+gte(t,0)*0.5*exp(-(t-0)*2.56)*sin(2*PI*1445*(t-0))+gte(t,0)*0.3*exp(-(t-0)*3.84)*sin(2*PI*2824*(t-0)))':s=48000:d=2.5"
-af "silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## hinweis-zwei-ton (0.7 s, gain 7.1 dB)

```
-f lavfi -i "aevalsrc='0.35*(gte(t,0)*exp(-(t-0)*7)*min(1,(t-0)*300)*(0.7*sin(2*PI*880.00*(t-0))+0.15*sin(2*PI*1760.00*(t-0)))+gte(t,0.13)*exp(-(t-0.13)*7)*min(1,(t-0.13)*300)*(0.7*sin(2*PI*1174.66*(t-0.13))+0.15*sin(2*PI*2349.32*(t-0.13))))':s=48000:d=0.7"
-af "silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## hinweis-drei-ton (0.9 s, gain 5.0 dB)

```
-f lavfi -i "aevalsrc='0.35*(gte(t,0)*exp(-(t-0)*6)*min(1,(t-0)*300)*(0.7*sin(2*PI*659.26*(t-0))+0.15*sin(2*PI*1318.51*(t-0)))+gte(t,0.11)*exp(-(t-0.11)*6)*min(1,(t-0.11)*300)*(0.7*sin(2*PI*783.99*(t-0.11))+0.15*sin(2*PI*1567.98*(t-0.11)))+gte(t,0.22)*exp(-(t-0.22)*6)*min(1,(t-0.22)*300)*(0.7*sin(2*PI*987.77*(t-0.22))+0.15*sin(2*PI*1975.53*(t-0.22))))':s=48000:d=0.9"
-af "silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## falsch-summer (0.5 s, gain 3.3 dB)

```
-f lavfi -i "aevalsrc='0.5*sgn(sin(2*PI*150*t))*(lt(t,0.14)+gte(t,0.18)*lt(t,0.32))':s=48000:d=0.5"
-af "lowpass=f=1800,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## erfolg-arpeggio (0.9 s, gain 4.2 dB)

```
-f lavfi -i "aevalsrc='0.35*(gte(t,0)*exp(-(t-0)*5)*min(1,(t-0)*300)*(0.6*sin(2*PI*523.25*(t-0))+0.2*sin(2*PI*1046.50*(t-0))+0.1*sin(2*PI*1569.75*(t-0)))+gte(t,0.08)*exp(-(t-0.08)*5)*min(1,(t-0.08)*300)*(0.6*sin(2*PI*659.26*(t-0.08))+0.2*sin(2*PI*1318.51*(t-0.08))+0.1*sin(2*PI*1977.77*(t-0.08)))+gte(t,0.16)*exp(-(t-0.16)*5)*min(1,(t-0.16)*300)*(0.6*sin(2*PI*783.99*(t-0.16))+0.2*sin(2*PI*1567.98*(t-0.16))+0.1*sin(2*PI*2351.97*(t-0.16)))+gte(t,0.24)*exp(-(t-0.24)*5)*min(1,(t-0.24)*300)*(0.6*sin(2*PI*1046.50*(t-0.24))+0.2*sin(2*PI*2093.00*(t-0.24))+0.1*sin(2*PI*3139.51*(t-0.24))))':s=48000:d=0.9"
-af "silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## kasse-glocke (1.4 s, gain 2.7 dB)

```
-f lavfi -i "aevalsrc='(random(0)*2-1)*exp(-t*90)*0.5+0.3*(gte(t,0.09)*1*exp(-(t-0.09)*3.50)*sin(2*PI*2093*(t-0.09))+gte(t,0.09)*0.7*exp(-(t-0.09)*4.55)*sin(2*PI*3136*(t-0.09))+gte(t,0.09)*0.4*exp(-(t-0.09)*6.30)*sin(2*PI*4186*(t-0.09)))':s=48000:d=1.4"
-af "highpass=f=150,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## kamera-ausloeser (0.3 s, gain 2.9 dB)

```
-f lavfi -i "aevalsrc='gte(t,0)*exp(-(t-0)*160)*((random(0)*2-1)*0.8+0.5*sin(2*PI*180*(t-0)))+gte(t,0.09)*exp(-(t-0.09)*160)*((random(0)*2-1)*0.8+0.5*sin(2*PI*180*(t-0.09)))':s=48000:d=0.3"
-af "bandpass=f=2500:width_type=h:w=4000,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## tippen (1.7 s, gain 3.1 dB)

```
-f lavfi -i "aevalsrc='gte(t,0)*exp(-(t-0)*220)*((random(0)*2-1)*0.7+0.5*sin(2*PI*220*(t-0)))+gte(t,0.17)*exp(-(t-0.17)*220)*((random(0)*2-1)*0.7+0.5*sin(2*PI*220*(t-0.17)))+gte(t,0.29)*exp(-(t-0.29)*220)*((random(0)*2-1)*0.7+0.5*sin(2*PI*220*(t-0.29)))+gte(t,0.52)*exp(-(t-0.52)*220)*((random(0)*2-1)*0.7+0.5*sin(2*PI*220*(t-0.52)))+gte(t,0.63)*exp(-(t-0.63)*220)*((random(0)*2-1)*0.7+0.5*sin(2*PI*220*(t-0.63)))+gte(t,0.82)*exp(-(t-0.82)*220)*((random(0)*2-1)*0.7+0.5*sin(2*PI*220*(t-0.82)))+gte(t,1.05)*exp(-(t-1.05)*220)*((random(0)*2-1)*0.7+0.5*sin(2*PI*220*(t-1.05)))+gte(t,1.17)*exp(-(t-1.17)*220)*((random(0)*2-1)*0.7+0.5*sin(2*PI*220*(t-1.17)))+gte(t,1.38)*exp(-(t-1.38)*220)*((random(0)*2-1)*0.7+0.5*sin(2*PI*220*(t-1.38)))+gte(t,1.5)*exp(-(t-1.5)*220)*((random(0)*2-1)*0.7+0.5*sin(2*PI*220*(t-1.5)))':s=48000:d=1.7"
-af "highpass=f=400,lowpass=f=6000,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## tippen-schnell (1.3 s, gain 3.1 dB)

```
-f lavfi -i "aevalsrc='gte(t,0)*exp(-(t-0)*240)*((random(0)*2-1)*0.7+0.5*sin(2*PI*260*(t-0)))+gte(t,0.07)*exp(-(t-0.07)*240)*((random(0)*2-1)*0.7+0.5*sin(2*PI*260*(t-0.07)))+gte(t,0.15)*exp(-(t-0.15)*240)*((random(0)*2-1)*0.7+0.5*sin(2*PI*260*(t-0.15)))+gte(t,0.2)*exp(-(t-0.2)*240)*((random(0)*2-1)*0.7+0.5*sin(2*PI*260*(t-0.2)))+gte(t,0.31)*exp(-(t-0.31)*240)*((random(0)*2-1)*0.7+0.5*sin(2*PI*260*(t-0.31)))+gte(t,0.38)*exp(-(t-0.38)*240)*((random(0)*2-1)*0.7+0.5*sin(2*PI*260*(t-0.38)))+gte(t,0.44)*exp(-(t-0.44)*240)*((random(0)*2-1)*0.7+0.5*sin(2*PI*260*(t-0.44)))+gte(t,0.58)*exp(-(t-0.58)*240)*((random(0)*2-1)*0.7+0.5*sin(2*PI*260*(t-0.58)))+gte(t,0.64)*exp(-(t-0.64)*240)*((random(0)*2-1)*0.7+0.5*sin(2*PI*260*(t-0.64)))+gte(t,0.75)*exp(-(t-0.75)*240)*((random(0)*2-1)*0.7+0.5*sin(2*PI*260*(t-0.75)))+gte(t,0.82)*exp(-(t-0.82)*240)*((random(0)*2-1)*0.7+0.5*sin(2*PI*260*(t-0.82)))+gte(t,0.9)*exp(-(t-0.9)*240)*((random(0)*2-1)*0.7+0.5*sin(2*PI*260*(t-0.9)))+gte(t,1.03)*exp(-(t-1.03)*240)*((random(0)*2-1)*0.7+0.5*sin(2*PI*260*(t-1.03)))+gte(t,1.1)*exp(-(t-1.1)*240)*((random(0)*2-1)*0.7+0.5*sin(2*PI*260*(t-1.1)))+gte(t,1.2)*exp(-(t-1.2)*240)*((random(0)*2-1)*0.7+0.5*sin(2*PI*260*(t-1.2)))':s=48000:d=1.3"
-af "highpass=f=500,lowpass=f=6500,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## maus-klick (0.18 s, gain -0.5 dB)

```
-f lavfi -i "aevalsrc='gte(t,0)*exp(-(t-0)*400)*((random(0)*2-1)*0.7+0.4*sin(2*PI*1800*(t-0)))+gte(t,0.07)*exp(-(t-0.07)*400)*((random(0)*2-1)*0.7+0.4*sin(2*PI*1800*(t-0.07)))':s=48000:d=0.18"
-af "highpass=f=800,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## schalter (0.25 s, gain 0.4 dB)

```
-f lavfi -i "aevalsrc='gte(t,0)*exp(-(t-0)*150)*((random(0)*2-1)*0.5+0.7*sin(2*PI*320*(t-0)))+gte(t,0.06)*exp(-(t-0.06)*150)*((random(0)*2-1)*0.5+0.7*sin(2*PI*320*(t-0.06)))':s=48000:d=0.25"
-af "lowpass=f=3500,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## tick-tack (3.0 s, gain -2.0 dB)

```
-f lavfi -i "aevalsrc='gte(t,0)*exp(-(t-0)*120)*(sin(2*PI*1800*(t-0))*0.8+(random(0)*2-1)*0.3)+gte(t,1)*exp(-(t-1)*120)*(sin(2*PI*1800*(t-1))*0.8+(random(0)*2-1)*0.3)+gte(t,2)*exp(-(t-2)*120)*(sin(2*PI*1800*(t-2))*0.8+(random(0)*2-1)*0.3)+gte(t,0.5)*exp(-(t-0.5)*120)*(sin(2*PI*1350*(t-0.5))*0.8+(random(0)*2-1)*0.3)+gte(t,1.5)*exp(-(t-1.5)*120)*(sin(2*PI*1350*(t-1.5))*0.8+(random(0)*2-1)*0.3)+gte(t,2.5)*exp(-(t-2.5)*120)*(sin(2*PI*1350*(t-2.5))*0.8+(random(0)*2-1)*0.3)':s=48000:d=3.0"
-af "highpass=f=500,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## herzschlag-schnell (1.8 s, gain -1.4 dB)

```
-f lavfi -i "aevalsrc='gte(t,0)*exp(-(t-0)*16)*(sin(2*PI*60*(t-0)))+gte(t,0.6)*exp(-(t-0.6)*16)*(sin(2*PI*60*(t-0.6)))+gte(t,1.2)*exp(-(t-1.2)*16)*(sin(2*PI*60*(t-1.2)))+gte(t,0.2)*exp(-(t-0.2)*20)*(0.7*sin(2*PI*55*(t-0.2)))+gte(t,0.8)*exp(-(t-0.8)*20)*(0.7*sin(2*PI*55*(t-0.8)))+gte(t,1.4)*exp(-(t-1.4)*20)*(0.7*sin(2*PI*55*(t-1.4)))':s=48000:d=1.8"
-af "lowpass=f=200,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## trommelwirbel (3.8 s, gain -2.0 dB)

```
-f lavfi -i "aevalsrc='gte(t,0.0)*exp(-(t-0.0)*35)*(((random(0)*2-1)*0.6+0.6*sin(2*PI*190*(t-0.0)))*(0.5+0.5*0.5))+gte(t,0.173)*exp(-(t-0.173)*35)*(((random(0)*2-1)*0.6+0.6*sin(2*PI*190*(t-0.173)))*(0.5+0.5*0.5))+gte(t,0.281)*exp(-(t-0.281)*35)*(((random(0)*2-1)*0.6+0.6*sin(2*PI*190*(t-0.281)))*(0.5+0.5*0.5))+gte(t,0.373)*exp(-(t-0.373)*35)*(((random(0)*2-1)*0.6+0.6*sin(2*PI*190*(t-0.373)))*(0.5+0.5*0.5))+gte(t,0.456)*exp(-(t-0.456)*35)*(((random(0)*2-1)*0.6+0.6*sin(2*PI*190*(t-0.456)))*(0.5+0.5*0.5))+gte(t,0.534)*exp(-(t-0.534)*35)*(((random(0)*2-1)*0.6+0.6*sin(2*PI*190*(t-0.534)))*(0.5+0.5*0.5))+gte(t,0.606)*exp(-(t-0.606)*35)*(((random(0)*2-1)*0.6+0.6*sin(2*PI*190*(t-0.606)))*(0.5+0.5*0.5))+gte(t,0.675)*exp(-(t-0.675)*35)*(((random(0)*2-1)*0.6+0.6*sin(2*PI*190*(t-0.675)))*(0.5+0.5*0.5))+gte(t,0.742)*exp(-(t-0.742)*35)*(((random(0)*2-1)*0.6+0.6*sin(2*PI*190*(t-0.742)))*(0.5+0.5*0.5))+gte(t,0.805)*exp(-(t-0.805)*35)*(((random(0)*2-1)*0.6+0.6*sin(2*PI*190*(t-0.805)))*(0.5+0.5*0.5))+gte(t,0.867)*exp(-(t-0.867)*35)*(((random(0)*2-1)*0.6+0.6*sin(2*PI*190*(t-0.867)))*(0.5+0.5*0.5))+gte(t,0.927)*exp(-(t-0.927)*35)*(((random(0)*2-1)*0.6+0.6*sin(2*PI*190*(t-0.927)))*(0.5+0.5*0.5))+gte(t,0.985)*exp(-(t-0.985)*35)*(((random(0)*2-1)*0.6+0.6*sin(2*PI*190*(t-0.985)))*(0.5+0.5*0.5))+gte(t,1.042)*exp(-(t-1.042)*35)*(((random(0)*2-1)*0.6+0.6*sin(2*PI*190*(t-1.042)))*(0.5+0.5*0.5))+gte(t,1.097)*exp(-(t-1.097)*35)*(((random(0)*2-1)*0.6+0.6*sin(2*PI*190*(t-1.097)))*(0.5+0.5*0.5))+gte(t,1.151)*exp(-(t-1.151)*35)*(((random(0)*2-1)*0.6+0.6*sin(2*PI*190*(t-1.151)))*(0.5+0.5*0.5))+gte(t,1.205)*exp(-(t-1.205)*35)*(((random(0)*2-1)*0.6+0.6*sin(2*PI*190*(t-1.205)))*(0.5+0.5*0.5))+gte(t,1.257)*exp(-(t-1.257)*35)*(((random(0)*2-1)*0.6+0.6*sin(2*PI*190*(t-1.257)))*(0.5+0.5*0.5))+gte(t,1.308)*exp(-(t-1.308)*35)*(((random(0)*2-1)*0.6+0.6*sin(2*PI*190*(t-1.308)))*(0.5+0.5*0.5))+gte(t,1.359)*exp(-(t-1.359)*35)*(((random(0)*2-1)*0.6+0.6*sin(2*PI*190*(t-1.359)))*(0.5+0.5*0.5))+gte(t,1.408)*exp(-(t-1.408)*35)*(((random(0)*2-1)*0.6+0.6*sin(2*PI*190*(t-1.408)))*(0.5+0.5*0.5))+gte(t,1.457)*exp(-(t-1.457)*35)*(((random(0)*2-1)*0.6+0.6*sin(2*PI*190*(t-1.457)))*(0.5+0.5*0.5))+gte(t,1.505)*exp(-(t-1.505)*35)*(((random(0)*2-1)*0.6+0.6*sin(2*PI*190*(t-1.505)))*(0.5+0.5*0.5))+gte(t,1.553)*exp(-(t-1.553)*35)*(((random(0)*2-1)*0.6+0.6*sin(2*PI*190*(t-1.553)))*(0.5+0.5*0.5))+gte(t,1.8)*exp(-(t-1.8)*2.6)*((random(0)*2-1)*0.9)':s=48000:d=3.8"
-af "highpass=f=120,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## spannung-schwellen (4.0 s, gain 0.1 dB)

```
-f lavfi -i "aevalsrc='(sin(2*PI*55*t)+0.6*sin(2*PI*55.7*t)+0.4*sin(2*PI*110.3*t))*0.4*min(1,t/2.5)*min(1,(4-t)/0.5)':s=48000:d=4.0"
-af "lowpass=f=600,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## streicher-spannung (3.0 s, gain 2.1 dB)

```
-f lavfi -i "aevalsrc='0.25*(sin(2*PI*(880*t+20*t*t))+sin(2*PI*(932*t+15*t*t))+0.5*sin(2*PI*(1318*t)))*min(1,t)*min(1,(3-t)/0.8)':s=48000:d=3.0"
-af "tremolo=f=6:d=0.25,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## sirene (2.4 s, gain 2.9 dB)

```
-f lavfi -i "aevalsrc='0.5*(sin(2*PI*(700+100*sgn(sin(PI*t/0.4)))*t)+0.3*sin(2*PI*2*(700+100*sgn(sin(PI*t/0.4)))*t))':s=48000:d=2.4"
-af "lowpass=f=3500,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## sonar-ping (1.6 s, gain 9.8 dB)

```
-f lavfi -i "aevalsrc='0.8*sin(2*PI*1000*t)*exp(-t*3)*min(1,t*500)':s=48000:d=1.6"
-af "aecho=0.6:0.5:300:0.4,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## treffer-punch (0.35 s, gain -2.0 dB)

```
-f lavfi -i "aevalsrc='sin(2*PI*(130*t-200*t*t))*exp(-t*14)+(random(0)*2-1)*exp(-t*70)*0.6':s=48000:d=0.35"
-af "lowpass=f=5000,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## treffer-knack (0.25 s, gain -2.0 dB)

```
-f lavfi -i "aevalsrc='(random(0)*2-1)*exp(-t*80)+0.5*sin(2*PI*2500*t)*exp(-t*60)':s=48000:d=0.25"
-af "highpass=f=1500,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## treffer-metall (1.0 s, gain -1.0 dB)

```
-f lavfi -i "aevalsrc='0.3*(gte(t,0)*1*exp(-(t-0)*5.00)*sin(2*PI*420*(t-0))+gte(t,0)*0.8*exp(-(t-0)*6.00)*sin(2*PI*638*(t-0))+gte(t,0)*0.6*exp(-(t-0)*7.50)*sin(2*PI*970*(t-0))+gte(t,0)*0.5*exp(-(t-0)*10.00)*sin(2*PI*1302*(t-0))+gte(t,0)*0.4*exp(-(t-0)*13.00)*sin(2*PI*1764*(t-0)))':s=48000:d=1.0"
-af "silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## treffer-glas (0.6 s, gain -0.6 dB)

```
-f lavfi -i "aevalsrc='0.3*(gte(t,0)*1*exp(-(t-0)*12.00)*sin(2*PI*2200*(t-0))+gte(t,0)*0.7*exp(-(t-0)*14.40)*sin(2*PI*3300*(t-0))+gte(t,0)*0.5*exp(-(t-0)*18.00)*sin(2*PI*4700*(t-0))+gte(t,0)*0.4*exp(-(t-0)*24.00)*sin(2*PI*6100*(t-0)))+(random(0)*2-1)*exp(-t*120)*0.3':s=48000:d=0.6"
-af "silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## treffer-dumpf (0.4 s, gain -1.6 dB)

```
-f lavfi -i "aevalsrc='sin(2*PI*70*t)*exp(-t*20)+(random(0)*2-1)*exp(-t*50)*0.4':s=48000:d=0.4"
-af "lowpass=f=800,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## treffer-klatsch (0.3 s, gain 1.8 dB)

```
-f lavfi -i "aevalsrc='(random(0)*2-1)*exp(-t*40)+0.4*sin(2*PI*400*t)*exp(-t*35)':s=48000:d=0.3"
-af "bandpass=f=3000:width_type=h:w=4500,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## explosion (3.0 s, gain -2.0 dB)

```
-f lavfi -i "aevalsrc='(random(0)*2-1)*exp(-t*2)*0.9+sin(2*PI*(70*t-8*t*t))*exp(-t*3)':s=48000:d=3.0"
-af "lowpass=f=2500,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## knall (0.9 s, gain 12.0 dB)

```
-f lavfi -i "aevalsrc='(random(0)*2-1)*exp(-t*15)':s=48000:d=0.9"
-af "lowpass=f=4000,aecho=0.7:0.5:80:0.3,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## doppelschlag (0.6 s, gain -2.0 dB)

```
-f lavfi -i "aevalsrc='gte(t,0)*exp(-(t-0)*16)*(sin(2*PI*(130*(t-0)-200*(t-0)*(t-0))))+gte(t,0.18)*exp(-(t-0.18)*16)*(sin(2*PI*(130*(t-0.18)-200*(t-0.18)*(t-0.18))))+gte(t,0)*exp(-(t-0)*70)*((random(0)*2-1)*0.5)+gte(t,0.18)*exp(-(t-0.18)*70)*((random(0)*2-1)*0.5)':s=48000:d=0.6"
-af "lowpass=f=5000,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## jingle-fanfare (2.2 s, gain -2.0 dB)

```
-f lavfi -i "aevalsrc='0.35*(gte(t,0)*exp(-(t-0)*2.5)*min(1,(t-0)*300)*(0.5*sin(2*PI*523.25*(t-0))+0.35*sin(2*PI*1046.50*(t-0))+0.2*sin(2*PI*1569.75*(t-0)))+gte(t,0.12)*exp(-(t-0.12)*2.5)*min(1,(t-0.12)*300)*(0.5*sin(2*PI*659.26*(t-0.12))+0.35*sin(2*PI*1318.51*(t-0.12))+0.2*sin(2*PI*1977.77*(t-0.12)))+gte(t,0.24)*exp(-(t-0.24)*2.5)*min(1,(t-0.24)*300)*(0.5*sin(2*PI*783.99*(t-0.24))+0.35*sin(2*PI*1567.98*(t-0.24))+0.2*sin(2*PI*2351.97*(t-0.24)))+gte(t,0.36)*exp(-(t-0.36)*2.5)*min(1,(t-0.36)*300)*(0.5*sin(2*PI*1046.50*(t-0.36))+0.35*sin(2*PI*2093.00*(t-0.36))+0.2*sin(2*PI*3139.51*(t-0.36)))+gte(t,0.6)*exp(-(t-0.6)*2.5)*min(1,(t-0.6)*300)*(0.5*sin(2*PI*523.25*(t-0.6))+0.35*sin(2*PI*1046.50*(t-0.6))+0.2*sin(2*PI*1569.75*(t-0.6)))+gte(t,0.6)*exp(-(t-0.6)*2.5)*min(1,(t-0.6)*300)*(0.5*sin(2*PI*659.26*(t-0.6))+0.35*sin(2*PI*1318.51*(t-0.6))+0.2*sin(2*PI*1977.77*(t-0.6)))+gte(t,0.6)*exp(-(t-0.6)*2.5)*min(1,(t-0.6)*300)*(0.5*sin(2*PI*783.99*(t-0.6))+0.35*sin(2*PI*1567.98*(t-0.6))+0.2*sin(2*PI*2351.97*(t-0.6)))+gte(t,0.6)*exp(-(t-0.6)*2.5)*min(1,(t-0.6)*300)*(0.5*sin(2*PI*1046.50*(t-0.6))+0.35*sin(2*PI*2093.00*(t-0.6))+0.2*sin(2*PI*3139.51*(t-0.6))))':s=48000:d=2.2"
-af "silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## jingle-erfolg (1.6 s, gain 1.0 dB)

```
-f lavfi -i "aevalsrc='0.35*(gte(t,0)*exp(-(t-0)*3.5)*min(1,(t-0)*300)*(0.6*sin(2*PI*392.00*(t-0))+0.25*sin(2*PI*783.99*(t-0))+0.1*sin(2*PI*1175.99*(t-0)))+gte(t,0.1)*exp(-(t-0.1)*3.5)*min(1,(t-0.1)*300)*(0.6*sin(2*PI*523.25*(t-0.1))+0.25*sin(2*PI*1046.50*(t-0.1))+0.1*sin(2*PI*1569.75*(t-0.1)))+gte(t,0.2)*exp(-(t-0.2)*3.5)*min(1,(t-0.2)*300)*(0.6*sin(2*PI*659.26*(t-0.2))+0.25*sin(2*PI*1318.51*(t-0.2))+0.1*sin(2*PI*1977.77*(t-0.2)))+gte(t,0.3)*exp(-(t-0.3)*3.5)*min(1,(t-0.3)*300)*(0.6*sin(2*PI*783.99*(t-0.3))+0.25*sin(2*PI*1567.98*(t-0.3))+0.1*sin(2*PI*2351.97*(t-0.3)))+gte(t,0.45)*exp(-(t-0.45)*3.5)*min(1,(t-0.45)*300)*(0.6*sin(2*PI*1046.50*(t-0.45))+0.25*sin(2*PI*2093.00*(t-0.45))+0.1*sin(2*PI*3139.51*(t-0.45)))+gte(t,0.45)*exp(-(t-0.45)*3.5)*min(1,(t-0.45)*300)*(0.6*sin(2*PI*1318.51*(t-0.45))+0.25*sin(2*PI*2637.02*(t-0.45))+0.1*sin(2*PI*3955.53*(t-0.45))))':s=48000:d=1.6"
-af "silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## jingle-aufstieg (1.6 s, gain 3.0 dB)

```
-f lavfi -i "aevalsrc='0.35*(gte(t,0)*exp(-(t-0)*4)*min(1,(t-0)*300)*(0.6*sin(2*PI*523.25*(t-0))+0.2*sin(2*PI*1046.50*(t-0))+0.1*sin(2*PI*1569.75*(t-0)))+gte(t,0.09)*exp(-(t-0.09)*4)*min(1,(t-0.09)*300)*(0.6*sin(2*PI*587.33*(t-0.09))+0.2*sin(2*PI*1174.66*(t-0.09))+0.1*sin(2*PI*1761.99*(t-0.09)))+gte(t,0.18)*exp(-(t-0.18)*4)*min(1,(t-0.18)*300)*(0.6*sin(2*PI*659.26*(t-0.18))+0.2*sin(2*PI*1318.51*(t-0.18))+0.1*sin(2*PI*1977.77*(t-0.18)))+gte(t,0.27)*exp(-(t-0.27)*4)*min(1,(t-0.27)*300)*(0.6*sin(2*PI*783.99*(t-0.27))+0.2*sin(2*PI*1567.98*(t-0.27))+0.1*sin(2*PI*2351.97*(t-0.27)))+gte(t,0.36)*exp(-(t-0.36)*4)*min(1,(t-0.36)*300)*(0.6*sin(2*PI*880.00*(t-0.36))+0.2*sin(2*PI*1760.00*(t-0.36))+0.1*sin(2*PI*2640.00*(t-0.36)))+gte(t,0.45)*exp(-(t-0.45)*4)*min(1,(t-0.45)*300)*(0.6*sin(2*PI*1046.50*(t-0.45))+0.2*sin(2*PI*2093.00*(t-0.45))+0.1*sin(2*PI*3139.51*(t-0.45))))':s=48000:d=1.6"
-af "silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## jingle-fehlschlag (1.8 s, gain 5.0 dB)

```
-f lavfi -i "aevalsrc='0.35*(gte(t,0)*exp(-(t-0)*3.2)*min(1,(t-0)*300)*(0.5*sin(2*PI*466.16*(t-0))+0.35*sin(2*PI*932.33*(t-0))+0.15*sin(2*PI*1398.49*(t-0)))+gte(t,0.2)*exp(-(t-0.2)*3.2)*min(1,(t-0.2)*300)*(0.5*sin(2*PI*392.00*(t-0.2))+0.35*sin(2*PI*783.99*(t-0.2))+0.15*sin(2*PI*1175.99*(t-0.2)))+gte(t,0.4)*exp(-(t-0.4)*3.2)*min(1,(t-0.4)*300)*(0.5*sin(2*PI*311.13*(t-0.4))+0.35*sin(2*PI*622.25*(t-0.4))+0.15*sin(2*PI*933.38*(t-0.4)))+gte(t,0.6)*exp(-(t-0.6)*3.2)*min(1,(t-0.6)*300)*(0.5*sin(2*PI*261.63*(t-0.6))+0.35*sin(2*PI*523.25*(t-0.6))+0.15*sin(2*PI*784.88*(t-0.6))))':s=48000:d=1.8"
-af "lowpass=f=3000,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## jingle-geheimnis (2.6 s, gain 14.9 dB)

```
-f lavfi -i "aevalsrc='0.35*(gte(t,0)*exp(-(t-0)*2.2)*min(1,(t-0)*300)*(0.7*sin(2*PI*659.26*(t-0))+0.1*sin(2*PI*1318.51*(t-0)))+gte(t,0.3)*exp(-(t-0.3)*2.2)*min(1,(t-0.3)*300)*(0.7*sin(2*PI*466.16*(t-0.3))+0.1*sin(2*PI*932.33*(t-0.3)))+gte(t,0.6)*exp(-(t-0.6)*2.2)*min(1,(t-0.6)*300)*(0.7*sin(2*PI*369.99*(t-0.6))+0.1*sin(2*PI*739.99*(t-0.6)))+gte(t,0.9)*exp(-(t-0.9)*2.2)*min(1,(t-0.9)*300)*(0.7*sin(2*PI*329.63*(t-0.9))+0.1*sin(2*PI*659.26*(t-0.9))))':s=48000:d=2.6"
-af "aecho=0.6:0.5:250:0.4,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## jingle-heiter (1.6 s, gain 5.1 dB)

```
-f lavfi -i "aevalsrc='0.35*(gte(t,0)*exp(-(t-0)*5)*min(1,(t-0)*300)*(0.6*sin(2*PI*523.25*(t-0))+0.25*sin(2*PI*1046.50*(t-0))+0.1*sin(2*PI*1569.75*(t-0)))+gte(t,0.12)*exp(-(t-0.12)*5)*min(1,(t-0.12)*300)*(0.6*sin(2*PI*659.26*(t-0.12))+0.25*sin(2*PI*1318.51*(t-0.12))+0.1*sin(2*PI*1977.77*(t-0.12)))+gte(t,0.24)*exp(-(t-0.24)*5)*min(1,(t-0.24)*300)*(0.6*sin(2*PI*783.99*(t-0.24))+0.25*sin(2*PI*1567.98*(t-0.24))+0.1*sin(2*PI*2351.97*(t-0.24)))+gte(t,0.36)*exp(-(t-0.36)*5)*min(1,(t-0.36)*300)*(0.6*sin(2*PI*659.26*(t-0.36))+0.25*sin(2*PI*1318.51*(t-0.36))+0.1*sin(2*PI*1977.77*(t-0.36)))+gte(t,0.48)*exp(-(t-0.48)*5)*min(1,(t-0.48)*300)*(0.6*sin(2*PI*523.25*(t-0.48))+0.25*sin(2*PI*1046.50*(t-0.48))+0.1*sin(2*PI*1569.75*(t-0.48)))+gte(t,0.6)*exp(-(t-0.6)*5)*min(1,(t-0.6)*300)*(0.6*sin(2*PI*783.99*(t-0.6))+0.25*sin(2*PI*1567.98*(t-0.6))+0.1*sin(2*PI*2351.97*(t-0.6)))+gte(t,0.72)*exp(-(t-0.72)*5)*min(1,(t-0.72)*300)*(0.6*sin(2*PI*1046.50*(t-0.72))+0.25*sin(2*PI*2093.00*(t-0.72))+0.1*sin(2*PI*3139.51*(t-0.72))))':s=48000:d=1.6"
-af "silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## jingle-abspann (3.5 s, gain -1.5 dB)

```
-f lavfi -i "aevalsrc='0.35*(gte(t,0)*exp(-(t-0)*1.2)*min(1,(t-0)*300)*(0.6*sin(2*PI*349.23*(t-0))+0.2*sin(2*PI*698.46*(t-0)))+gte(t,0)*exp(-(t-0)*1.2)*min(1,(t-0)*300)*(0.6*sin(2*PI*440.00*(t-0))+0.2*sin(2*PI*880.00*(t-0)))+gte(t,0)*exp(-(t-0)*1.2)*min(1,(t-0)*300)*(0.6*sin(2*PI*523.25*(t-0))+0.2*sin(2*PI*1046.50*(t-0)))+gte(t,1.2)*exp(-(t-1.2)*1.2)*min(1,(t-1.2)*300)*(0.6*sin(2*PI*261.63*(t-1.2))+0.2*sin(2*PI*523.25*(t-1.2)))+gte(t,1.2)*exp(-(t-1.2)*1.2)*min(1,(t-1.2)*300)*(0.6*sin(2*PI*329.63*(t-1.2))+0.2*sin(2*PI*659.26*(t-1.2)))+gte(t,1.2)*exp(-(t-1.2)*1.2)*min(1,(t-1.2)*300)*(0.6*sin(2*PI*392.00*(t-1.2))+0.2*sin(2*PI*783.99*(t-1.2)))+gte(t,1.2)*exp(-(t-1.2)*1.2)*min(1,(t-1.2)*300)*(0.6*sin(2*PI*523.25*(t-1.2))+0.2*sin(2*PI*1046.50*(t-1.2))))':s=48000:d=3.5"
-af "silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## jingle-glockenspiel (2.0 s, gain 3.0 dB)

```
-f lavfi -i "aevalsrc='0.35*(gte(t,0)*exp(-(t-0)*3)*min(1,(t-0)*300)*(0.5*sin(2*PI*1046.50*(t-0))+0.15*sin(2*PI*2093.00*(t-0))+0.3*sin(2*PI*3139.51*(t-0)))+gte(t,0.09)*exp(-(t-0.09)*3)*min(1,(t-0.09)*300)*(0.5*sin(2*PI*1318.51*(t-0.09))+0.15*sin(2*PI*2637.02*(t-0.09))+0.3*sin(2*PI*3955.53*(t-0.09)))+gte(t,0.18)*exp(-(t-0.18)*3)*min(1,(t-0.18)*300)*(0.5*sin(2*PI*1567.98*(t-0.18))+0.15*sin(2*PI*3135.96*(t-0.18))+0.3*sin(2*PI*4703.95*(t-0.18)))+gte(t,0.27)*exp(-(t-0.27)*3)*min(1,(t-0.27)*300)*(0.5*sin(2*PI*2093.00*(t-0.27))+0.15*sin(2*PI*4186.01*(t-0.27))+0.3*sin(2*PI*6279.01*(t-0.27))))':s=48000:d=2.0"
-af "silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## jingle-quiz-richtig (0.9 s, gain 6.7 dB)

```
-f lavfi -i "aevalsrc='0.35*(gte(t,0)*exp(-(t-0)*5)*min(1,(t-0)*300)*(0.6*sin(2*PI*1318.51*(t-0))+0.2*sin(2*PI*2637.02*(t-0)))+gte(t,0.12)*exp(-(t-0.12)*5)*min(1,(t-0.12)*300)*(0.6*sin(2*PI*1760.00*(t-0.12))+0.2*sin(2*PI*3520.00*(t-0.12))))':s=48000:d=0.9"
-af "silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## jingle-level-geschafft (2.6 s, gain 0.9 dB)

```
-f lavfi -i "aevalsrc='0.35*(gte(t,0)*exp(-(t-0)*2.6)*min(1,(t-0)*300)*(0.5*sin(2*PI*523.25*(t-0))+0.3*sin(2*PI*1046.50*(t-0))+0.1*sin(2*PI*1569.75*(t-0)))+gte(t,0.1)*exp(-(t-0.1)*2.6)*min(1,(t-0.1)*300)*(0.5*sin(2*PI*587.33*(t-0.1))+0.3*sin(2*PI*1174.66*(t-0.1))+0.1*sin(2*PI*1761.99*(t-0.1)))+gte(t,0.2)*exp(-(t-0.2)*2.6)*min(1,(t-0.2)*300)*(0.5*sin(2*PI*659.26*(t-0.2))+0.3*sin(2*PI*1318.51*(t-0.2))+0.1*sin(2*PI*1977.77*(t-0.2)))+gte(t,0.3)*exp(-(t-0.3)*2.6)*min(1,(t-0.3)*300)*(0.5*sin(2*PI*783.99*(t-0.3))+0.3*sin(2*PI*1567.98*(t-0.3))+0.1*sin(2*PI*2351.97*(t-0.3)))+gte(t,0.4)*exp(-(t-0.4)*2.6)*min(1,(t-0.4)*300)*(0.5*sin(2*PI*1046.50*(t-0.4))+0.3*sin(2*PI*2093.00*(t-0.4))+0.1*sin(2*PI*3139.51*(t-0.4)))+gte(t,0.65)*exp(-(t-0.65)*2.6)*min(1,(t-0.65)*300)*(0.5*sin(2*PI*783.99*(t-0.65))+0.3*sin(2*PI*1567.98*(t-0.65))+0.1*sin(2*PI*2351.97*(t-0.65)))+gte(t,0.65)*exp(-(t-0.65)*2.6)*min(1,(t-0.65)*300)*(0.5*sin(2*PI*1046.50*(t-0.65))+0.3*sin(2*PI*2093.00*(t-0.65))+0.1*sin(2*PI*3139.51*(t-0.65)))+gte(t,0.65)*exp(-(t-0.65)*2.6)*min(1,(t-0.65)*300)*(0.5*sin(2*PI*1318.51*(t-0.65))+0.3*sin(2*PI*2637.02*(t-0.65))+0.1*sin(2*PI*3955.53*(t-0.65))))':s=48000:d=2.6"
-af "silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## tuer-klopfen (0.8 s, gain -1.6 dB)

```
-f lavfi -i "aevalsrc='gte(t,0)*exp(-(t-0)*45)*(sin(2*PI*130*(t-0)))+gte(t,0.22)*exp(-(t-0.22)*45)*(sin(2*PI*130*(t-0.22)))+gte(t,0.44)*exp(-(t-0.44)*45)*(sin(2*PI*130*(t-0.44)))+gte(t,0)*exp(-(t-0)*90)*((random(0)*2-1)*0.3)+gte(t,0.22)*exp(-(t-0.22)*90)*((random(0)*2-1)*0.3)+gte(t,0.44)*exp(-(t-0.44)*90)*((random(0)*2-1)*0.3)':s=48000:d=0.8"
-af "lowpass=f=900,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## schritte (2.9 s, gain 0.6 dB)

```
-f lavfi -i "aevalsrc='gte(t,0)*exp(-(t-0)*28)*((random(0)*2-1)*0.7+0.7*sin(2*PI*95*(t-0)))+gte(t,0.9)*exp(-(t-0.9)*28)*((random(0)*2-1)*0.7+0.7*sin(2*PI*95*(t-0.9)))+gte(t,1.8)*exp(-(t-1.8)*28)*((random(0)*2-1)*0.7+0.7*sin(2*PI*95*(t-1.8)))+gte(t,0.45)*exp(-(t-0.45)*28)*((random(0)*2-1)*0.6+0.7*sin(2*PI*80*(t-0.45)))+gte(t,1.35)*exp(-(t-1.35)*28)*((random(0)*2-1)*0.6+0.7*sin(2*PI*80*(t-1.35)))+gte(t,2.25)*exp(-(t-2.25)*28)*((random(0)*2-1)*0.6+0.7*sin(2*PI*80*(t-2.25)))':s=48000:d=2.9"
-af "lowpass=f=700,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## wassertropfen (2.2 s, gain 12.1 dB)

```
-f lavfi -i "aevalsrc='gte(t,0)*exp(-(t-0)*40)*(sin(2*PI*(800*(t-0)+4000*(t-0)*(t-0))))+gte(t,0.7)*exp(-(t-0.7)*40)*(sin(2*PI*(800*(t-0.7)+4000*(t-0.7)*(t-0.7))))+gte(t,1.35)*exp(-(t-1.35)*40)*(sin(2*PI*(800*(t-1.35)+4000*(t-1.35)*(t-1.35))))':s=48000:d=2.2"
-af "aecho=0.5:0.4:120:0.3,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## tuerklingel (2.0 s, gain 4.8 dB)

```
-f lavfi -i "aevalsrc='0.5*(gte(t,0)*exp(-(t-0)*3)*min(1,(t-0)*300)*(0.6*sin(2*PI*880.00*(t-0))+0.25*sin(2*PI*1760.00*(t-0))+0.1*sin(2*PI*2640.00*(t-0))))+0.5*(gte(t,0.5)*exp(-(t-0.5)*3)*min(1,(t-0.5)*300)*(0.6*sin(2*PI*698.00*(t-0.5))+0.25*sin(2*PI*1396.00*(t-0.5))+0.1*sin(2*PI*2094.00*(t-0.5))))':s=48000:d=2.0"
-af "silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## wecker-klingeln (2.0 s, gain -1.1 dB)

```
-f lavfi -i "aevalsrc='0.6*sin(2*PI*1800*t)*lt(mod(t,0.12),0.06)*lt(mod(t,1),0.6)*(1+0.5*sin(2*PI*60*t))':s=48000:d=2.0"
-af "silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## papier-rascheln (1.0 s, gain -0.5 dB)

```
-f lavfi -i "aevalsrc='gte(t,0)*exp(-(t-0)*60)*((random(0)*2-1))+gte(t,0.06)*exp(-(t-0.06)*60)*((random(0)*2-1))+gte(t,0.15)*exp(-(t-0.15)*60)*((random(0)*2-1))+gte(t,0.22)*exp(-(t-0.22)*60)*((random(0)*2-1))+gte(t,0.38)*exp(-(t-0.38)*60)*((random(0)*2-1))+gte(t,0.47)*exp(-(t-0.47)*60)*((random(0)*2-1))+gte(t,0.6)*exp(-(t-0.6)*60)*((random(0)*2-1))+gte(t,0.71)*exp(-(t-0.71)*60)*((random(0)*2-1))':s=48000:d=1.0"
-af "bandpass=f=5000:width_type=h:w=5000,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## korken-plopp (0.3 s, gain -2.0 dB)

```
-f lavfi -i "aevalsrc='sin(2*PI*(250*t+450*0.02*(1-exp(-t/0.02))))*exp(-t*25)+(random(0)*2-1)*exp(-t*150)*0.3':s=48000:d=0.3"
-af "silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## klatschen-einzeln (0.3 s, gain 0.9 dB)

```
-f lavfi -i "aevalsrc='gte(t,0)*exp(-(t-0)*40)*((random(0)*2-1))+gte(t,0.012)*exp(-(t-0.012)*40)*((random(0)*2-1))+gte(t,0.022)*exp(-(t-0.022)*40)*((random(0)*2-1))':s=48000:d=0.3"
-af "bandpass=f=1500:width_type=h:w=2500,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## fingerschnipsen (0.2 s, gain 1.7 dB)

```
-f lavfi -i "aevalsrc='(random(0)*2-1)*exp(-t*90)+0.5*sin(2*PI*1100*t)*exp(-t*60)':s=48000:d=0.2"
-af "bandpass=f=2800:width_type=h:w=3000,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## gong (4.0 s, gain -0.5 dB)

```
-f lavfi -i "aevalsrc='0.35*(gte(t,0)*1*exp(-(t-0)*0.90)*sin(2*PI*196*(t-0))+gte(t,0)*0.6*exp(-(t-0)*1.17)*sin(2*PI*294*(t-0))+gte(t,0)*0.5*exp(-(t-0)*1.53)*sin(2*PI*392*(t-0))+gte(t,0)*0.4*exp(-(t-0)*1.98)*sin(2*PI*570*(t-0))+gte(t,0)*0.3*exp(-(t-0)*2.70)*sin(2*PI*801*(t-0)))':s=48000:d=4.0"
-af "silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## dampf-zischen (1.5 s, gain -2.0 dB)

```
-f lavfi -i "aevalsrc='(random(0)*2-1)*sin(PI*t/1.5)*0.7':s=48000:d=1.5"
-af "highpass=f=3000,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## reissverschluss (0.8 s, gain 0.5 dB)

```
-f lavfi -i "aevalsrc='(random(0)*2-1)*lt(mod(t*(1+0.6*t),0.012),0.004)':s=48000:d=0.8"
-af "bandpass=f=3000:width_type=h:w=3000,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## platsch (1.2 s, gain 5.0 dB)

```
-f lavfi -i "aevalsrc='(random(0)*2-1)*exp(-t*9)*0.8':s=48000:d=1.2"
-af "bandpass=f=1800:width_type=h:w=2500,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## powerup (0.8 s, gain 6.3 dB)

```
-f lavfi -i "aevalsrc='0.35*sgn(sin(2*PI*(262*pow(2,floor(t*14)/6))*t))*lt(t,0.57)*exp(-mod(t,1/14)*4)':s=48000:d=0.8"
-af "lowpass=f=4500,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## game-over (1.8 s, gain 7.7 dB)

```
-f lavfi -i "aevalsrc='0.3*sgn(sin(2*PI*if(lt(t,0.3),392,if(lt(t,0.6),330,if(lt(t,0.9),262,196)))*t))*exp(-mod(t,0.3)*3)*lt(t,1.5)':s=48000:d=1.8"
-af "lowpass=f=3000,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## checkpoint (0.7 s, gain 6.9 dB)

```
-f lavfi -i "aevalsrc='0.35*(gte(t,0)*exp(-(t-0)*7)*min(1,(t-0)*300)*(0.7*sin(2*PI*1318.51*(t-0))+0.1*sin(2*PI*2637.02*(t-0)))+gte(t,0.1)*exp(-(t-0.1)*7)*min(1,(t-0.1)*300)*(0.7*sin(2*PI*1975.53*(t-0.1))+0.1*sin(2*PI*3951.07*(t-0.1))))':s=48000:d=0.7"
-af "silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## menue-blip (0.12 s, gain 3.2 dB)

```
-f lavfi -i "aevalsrc='0.5*sgn(sin(2*PI*1000*t))*lt(t,0.05)':s=48000:d=0.12"
-af "lowpass=f=3500,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## menue-bestaetigung (0.2 s, gain 3.2 dB)

```
-f lavfi -i "aevalsrc='0.5*sgn(sin(2*PI*if(lt(t,0.06),900,1350)*t))*lt(t,0.12)':s=48000:d=0.2"
-af "lowpass=f=3500,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## portal (1.6 s, gain 12.0 dB)

```
-f lavfi -i "aevalsrc='0.4*sin(2*PI*(300*t+200*t*t+25*sin(2*PI*7*t)))*sin(PI*t/1.6)':s=48000:d=1.6"
-af "aecho=0.6:0.5:60:0.4,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## scanner (1.2 s, gain 7.1 dB)

```
-f lavfi -i "aevalsrc='0.35*sin(2*PI*(400*t+800*t*t)+4*sin(2*PI*30*t))*sin(PI*t/1.2)':s=48000:d=1.2"
-af "tremolo=f=12:d=0.4,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## laser-salve (0.8 s, gain 7.6 dB)

```
-f lavfi -i "aevalsrc='0.3*sgn(sin(2*PI*(2200*mod(t,0.2)-3500*mod(t,0.2)*mod(t,0.2))))*exp(-mod(t,0.2)*12)*lt(t,0.6)':s=48000:d=0.8"
-af "lowpass=f=5000,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## schatztruhe (1.8 s, gain 3.4 dB)

```
-f lavfi -i "aevalsrc='0.35*(gte(t,0)*exp(-(t-0)*3)*min(1,(t-0)*300)*(0.6*sin(2*PI*1046.50*(t-0))+0.2*sin(2*PI*2093.00*(t-0)))+gte(t,0.08)*exp(-(t-0.08)*3)*min(1,(t-0.08)*300)*(0.6*sin(2*PI*1318.51*(t-0.08))+0.2*sin(2*PI*2637.02*(t-0.08)))+gte(t,0.17)*exp(-(t-0.17)*3)*min(1,(t-0.17)*300)*(0.6*sin(2*PI*1567.98*(t-0.17))+0.2*sin(2*PI*3135.96*(t-0.17)))+gte(t,0.3)*exp(-(t-0.3)*3)*min(1,(t-0.3)*300)*(0.6*sin(2*PI*2093.00*(t-0.3))+0.2*sin(2*PI*4186.01*(t-0.3)))+gte(t,0.42)*exp(-(t-0.42)*3)*min(1,(t-0.42)*300)*(0.6*sin(2*PI*1318.51*(t-0.42))+0.2*sin(2*PI*2637.02*(t-0.42)))+gte(t,0.58)*exp(-(t-0.58)*3)*min(1,(t-0.58)*300)*(0.6*sin(2*PI*1567.98*(t-0.58))+0.2*sin(2*PI*3135.96*(t-0.58)))+gte(t,0.7)*exp(-(t-0.7)*3)*min(1,(t-0.7)*300)*(0.6*sin(2*PI*2093.00*(t-0.7))+0.2*sin(2*PI*4186.01*(t-0.7)))+gte(t,0.9)*exp(-(t-0.9)*3)*min(1,(t-0.9)*300)*(0.6*sin(2*PI*2637.02*(t-0.9))+0.2*sin(2*PI*5274.04*(t-0.9))))':s=48000:d=1.8"
-af "silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## zaehler (1.0 s, gain 3.2 dB)

```
-f lavfi -i "aevalsrc='0.5*sgn(sin(2*PI*1200*t))*lt(mod(t*(1+t*2),0.06),0.015)':s=48000:d=1.0"
-af "lowpass=f=4000,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## roboter-piepen (0.8 s, gain 6.0 dB)

```
-f lavfi -i "aevalsrc='0.4*sin(2*PI*if(lt(t,0.15),1200,if(lt(t,0.3),1800,if(lt(t,0.45),900,1500)))*t)*lt(mod(t,0.15),0.12)*lt(t,0.6)':s=48000:d=0.8"
-af "silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## rutschpfeife-auf (1.0 s, gain 4.0 dB)

```
-f lavfi -i "aevalsrc='0.5*sin(2*PI*(400*t+800*t*t)+3*sin(2*PI*6*t))*min(1,(1-t)*10)':s=48000:d=1.0"
-af "silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## rutschpfeife-ab (1.0 s, gain 4.0 dB)

```
-f lavfi -i "aevalsrc='0.5*sin(2*PI*(1600*t-800*t*t)+3*sin(2*PI*6*t))*min(1,(1-t)*10)':s=48000:d=1.0"
-af "silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## quietscher (0.25 s, gain 4.0 dB)

```
-f lavfi -i "aevalsrc='0.5*sin(2*PI*(1500*t+2000*t*t))*exp(-t*8)':s=48000:d=0.25"
-af "silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```

## holz-bonk (0.3 s, gain -1.9 dB)

```
-f lavfi -i "aevalsrc='sin(2*PI*(320*t-300*t*t))*exp(-t*22)':s=48000:d=0.3"
-af "lowpass=f=3000,silenceremove=start_periods=1:start_threshold=-50dB:detection=peak,areverse,afade=t=in:d=0.025,areverse"
```
