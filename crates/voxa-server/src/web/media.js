// Private video uses the same bounded JPEG frames and video-note format as Voxa.
// The browser's capture prompt is entered only from a user action. No STUN/TURN
// or peer addresses: the signed-in server checks both participants on every hop.
'use strict';
globalThis.VoxaMedia = function ({el, dialog, send, T, report, openDm}) {
  const fps = 8, maxFrames = 120, maxBytes = 8 * 1024 * 1024;
  let local = null, incoming = null;
  const u32 = n => { const b = new Uint8Array(4); new DataView(b.buffer).setUint32(0, n, true); return b; };
  const post = async (path, body) => {
    const r = await fetch(path, {method:'POST', headers:{'X-Voxa':'1'}, body});
    if (!r.ok) throw new Error((await r.text()).slice(0, 160) || 'Media unavailable');
  };
  function release(s) {
    clearTimeout(s.timer); clearTimeout(s.deadline);
    s.capture?.getTracks().forEach(t => t.stop());
    s.processor?.disconnect(); s.audio?.close().catch(() => {});
    if (s.url) URL.revokeObjectURL(s.url);
  }
  function stop(s, notify = true) {
    if (!s || s.closed) return;
    s.closed = true; release(s);
    if (notify && !s.record && s.offered) send({EndMedia:{peer:s.peer, stream:s.id}});
    if (s === local) local = null;
    if (s === incoming) incoming = null;
    s.view?.close();
  }
  const picture = canvas => new Promise((resolve,reject) => canvas.toBlob(b => b ? resolve(b) : reject(new Error('Camera image unavailable')), 'image/jpeg', 0.65));
  async function start(uid, peer, camera, record, prefs) {
    if (local) { report('Close your current preview first'); return; }
    if (!navigator.mediaDevices) { report('Camera and screen capture need HTTPS or localhost'); return; }
    const s = local = {uid, peer, record, camera, id:crypto.getRandomValues(new Uint32Array(1))[0], frames:[], pcm:[], bytes:0, samples:0};
    const info = el('p', null, record ? 'Recording a private video message (up to 15 seconds).' : 'Waiting for the other person to accept…');
    const video = el('video', {muted:true, playsinline:true}); video.muted = true;
    const canvas = el('canvas',{class:'dm-preview'});
    const src = document.createElement('canvas'), raw = src.getContext('2d',{willReadFrequently:true});
    const out = canvas.getContext('2d');
    const finish = () => {
      s.finished = true; release(s); info.textContent = s.frames.length ? 'Recording stopped. Send or discard the video message.' : 'No frames recorded.';
      return false;
    };
    async function sendNote() {
      if (!s.finished) finish();
      if (!s.frames.length || s.sending) return false;
      s.sending = true;
      try {
        const audio = new Blob(s.pcm).slice(0,s.frames.length * 12000);
        const file = new Blob(['VXC1',u32(s.frames.length),u32(audio.size),...s.frames.flatMap(b=>[u32(b.size),b]),audio]);
        if (file.size > maxBytes) throw new Error('Video too large');
        await post('/api/media/clip?uid='+encodeURIComponent(uid),file);
        stop(s,false); return false;
      } catch (e) { info.textContent=e.message; s.sending=false; return false; }
    }
    s.view = dialog(record ? T('Video message') : camera?T('Private camera'):T('Private screen share'), el('div',null,canvas,info),
      record ? [{label:T('Stop recording'),onClick:finish},{label:T('Send'),onClick:sendNote},{label:T('Discard')}] : [{label:T('Close')}], {onClose:()=>stop(s)});
    try {
      const videoConstraints = {width:{ideal:640},height:{ideal:360}};
      let capture;
      if (camera) {
        try { capture = await navigator.mediaDevices.getUserMedia({video:videoConstraints, audio:record}); }
        catch (e) {
          if (!record || e.name !== 'NotFoundError' || s.closed || s.finished) throw e;
          capture = await navigator.mediaDevices.getUserMedia({video:videoConstraints, audio:false});
          info.textContent = 'Recording video without a microphone.';
        }
      } else {
        capture = await navigator.mediaDevices.getDisplayMedia({video:true,audio:false});
      }
      if (s.closed || s.finished) { capture.getTracks().forEach(t=>t.stop()); return; }
      s.capture=capture;
      capture.getVideoTracks()[0].onended=()=>record ? finish() : stop(s);
      video.srcObject=capture; await video.play();
      if (s.closed || s.finished) return;
      if (record && capture.getAudioTracks().length) {
        const audio=s.audio=new AudioContext({sampleRate:48000});
        const mic=audio.createMediaStreamSource(capture), p=s.processor=audio.createScriptProcessor(2048,1,1);
        mic.connect(p); p.connect(audio.destination);
        p.onaudioprocess = e => {
          if (s.closed || s.finished || s.samples>=720000) return;
          const a=e.inputBuffer.getChannelData(0), data=new DataView(new ArrayBuffer(a.length*2));
          for(let i=0;i<a.length;i++) data.setInt16(i*2,Math.round(Math.min(1,Math.max(-1,a[i]))*32767),true);
          s.samples+=a.length;s.pcm.push(data.buffer);
        };
      }
      if (!record) {
        send({OfferMedia:{to:peer,stream:s.id,camera}}); s.offered=true;
        s.deadline=setTimeout(()=>{if (!s.accepted) stop(s);},60000);
      }
      const startTime=performance.now(), angle=camera ? prefs.rotation : 0, effect=camera ? prefs.background : 'Off';
      let reference, backdrop;
      const tick=async () => {
        if (s.closed || s.finished) return;
        try {
          const w=Math.max(1,video.videoWidth), h=Math.max(1,video.videoHeight), k=Math.min(1,(record?480:960)/Math.max(w,h));
          const sw=Math.max(1,Math.round(w*k)), sh=Math.max(1,Math.round(h*k));
          if (src.width!==sw || src.height!==sh) {src.width=sw;src.height=sh;reference=null;}
          raw.drawImage(video,0,0,sw,sh);
          if (effect!=='Off') {
            if (!reference && performance.now()-startTime>=3000) {
              reference=raw.getImageData(0,0,sw,sh);
              if (effect==='Blur') {
                const small=document.createElement('canvas'); small.width=Math.max(1,sw>>4); small.height=Math.max(1,sh>>4);
                small.getContext('2d').drawImage(src,0,0,small.width,small.height);
                raw.drawImage(small,0,0,sw,sh);
              } else if (effect==='Picture' && prefs.picture) {raw.drawImage(prefs.picture,0,0,sw,sh);}
              else {raw.fillStyle='#293d54';raw.fillRect(0,0,sw,sh);}
              backdrop=raw.getImageData(0,0,sw,sh);raw.putImageData(backdrop,0,0);
            } else if (!reference) {raw.fillStyle='#293d54';raw.fillRect(0,0,sw,sh);}
            else {
              const f=raw.getImageData(0,0,sw,sh), p=f.data, r=reference.data, b=backdrop.data;
              for(let i=0;i<p.length;i+=4) {
                const diff=Math.max(Math.abs(p[i]-r[i]),Math.abs(p[i+1]-r[i+1]),Math.abs(p[i+2]-r[i+2]));
                const a=Math.min(1,Math.max(0,(diff-16)/30));
                for(let c=0;c<3;c++) p[i+c]=a*p[i+c]+(1-a)*b[i+c];
              }
              raw.putImageData(f,0,0);
            }
          }
          canvas.width=angle%2 ? sh : sw;canvas.height=angle%2 ? sw : sh;
          out.translate(canvas.width/2,canvas.height/2);out.rotate(angle*Math.PI/2);out.drawImage(src,-sw/2,-sh/2);
          const blob=await picture(canvas);
          if (s.closed || s.finished) return;
          if (blob.size<=240*1024) {
            if (record) {
              if(s.bytes+blob.size>maxBytes-1500000 || s.frames.length>=maxFrames){finish();return;}
              s.frames.push(blob);s.bytes+=blob.size;
            } else if(s.accepted) {await post(`/api/media/frame?peer=${peer}&stream=${s.id}`,blob);}
          }
          if(record && performance.now()-startTime>=15000){finish();return;}
          s.timer=setTimeout(tick,1000/fps);
        } catch(e){report(e.message);stop(s);}
      };
      tick();
    } catch(e){report(e.message);stop(s);}
  }
  async function watch(s) {
    if(s.closed) return;
    try {
      const res=await fetch(`/api/media/frame?peer=${s.peer}&stream=${s.id}&after=${s.seq}`,{headers:{'X-Voxa':'1'}});
      if(!res.ok) throw new Error('Private video ended');
      if(res.status!==204) {
        const b=await res.blob();
        if(s.closed)return;
        if(s.url)URL.revokeObjectURL(s.url);
        s.url=URL.createObjectURL(b);s.img.src=s.url;s.seq=Number(res.headers.get('X-Sequence'));
      }
      if(!s.closed)s.timer=setTimeout(()=>watch(s),1000/fps);
    } catch(e){report(e.message);stop(s);}
  }
  async function playClip(id) {
    const img=el('img',{class:'dm-preview',alt:'Video message'}), s={};
    const audio = new AudioContext(); await audio.resume();
    dialog(T('Video message'),img,[{label:T('Close')}],{onClose:()=>{s.closed=true;clearTimeout(s.timer);if(s.url)URL.revokeObjectURL(s.url);audio.close();}});
    try {
      const r=await fetch('/api/media/clip?id='+encodeURIComponent(id),{headers:{'X-Voxa':'1'}});if(!r.ok)throw new Error('Video unavailable');
      const b=await r.arrayBuffer(); if(s.closed)return;
      if(b.byteLength>maxBytes || b.byteLength<12 || new TextDecoder().decode(b.slice(0,4))!=='VXC1')throw new Error('Invalid video message');
      const v=new DataView(b), count=v.getUint32(4,true), pcm=v.getUint32(8,true), frames=[];let pos=12;
      if(!count||count>maxFrames||pcm>count*12000||pcm%2)throw new Error('Invalid video length');
      for(let i=0;i<count;i++){const n=v.getUint32(pos,true);pos+=4;if(n>240*1024||pos+n>b.byteLength)throw new Error('Invalid frame');frames.push(new Blob([new Uint8Array(b,pos,n)],{type:'image/jpeg'}));pos+=n;}
      if(pos+pcm!==b.byteLength)throw new Error('Invalid sound length');
      if(pcm){
        const buf=audio.createBuffer(1,pcm/2,48000), ch=buf.getChannelData(0);
        for(let i=0;i<ch.length;i++)ch[i]=v.getInt16(pos+2*i,true)/32768;
        const source=audio.createBufferSource();source.buffer=buf;source.connect(audio.destination);source.start();
      }
      const began=performance.now();
      const draw=()=>{const i=Math.floor((performance.now()-began)*fps/1000);if(s.closed||i>=count)return;
        if(s.url)URL.revokeObjectURL(s.url);s.url=URL.createObjectURL(frames[i]);img.src=s.url;s.timer=setTimeout(draw,1000/fps);};draw();
    }catch(e){report(e.message);}
  }
  return {
    start, playClip, close(uid){for(const s of [local,incoming])if(s && (!uid || s.uid===uid))stop(s);},
    offer(v,person) {
      if(!person || incoming){send({AnswerMedia:{from:v.from,stream:v.stream,accept:false}});return;}
      openDm(person);const s=incoming={uid:person.uid,peer:v.from,id:v.stream,offered:true,seq:0};
      s.img=el('img',{class:'dm-preview',alt:'Private video'});
      s.view=dialog(person.nickname+' · '+(v.camera?T('Private camera'):T('Private screen share')),s.img,[
        {label:T('Accept'),onClick:()=>{if(!s.accepted)send({AnswerMedia:{from:s.peer,stream:s.id,accept:true}});return false;}},
        {label:T('Close')}],{onClose:()=>stop(s)});
      s.deadline=setTimeout(()=>{if(!s.accepted)stop(s);},60000);
    },
    answer(v){for(const s of [local,incoming])if(s && s.peer===v.peer && s.id===v.stream && !s.accepted){
      if(!v.accept){stop(s,false);continue;} s.accepted=true;if(s===incoming)watch(s);
      else {const p=s.view.root.querySelector('p');if(p)p.textContent='Private share is live';}
    }},
    ended(v){for(const s of [local,incoming])if(s && s.peer===v.peer && s.id===v.stream)stop(s,false);},
  };
};
