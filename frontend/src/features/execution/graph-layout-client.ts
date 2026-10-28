import type { LayoutInput, LayoutResult } from './graph-layout';

type LayoutWorker = Pick<Worker, 'postMessage' | 'terminate'> & {
  onmessage: ((event: MessageEvent<{id:number;result?:LayoutResult;error?:string}>)=>void) | null;
  onerror: ((event: ErrorEvent)=>void) | null;
};
let sequence = 0;
function validResult(input: LayoutInput, value: unknown): value is LayoutResult {
  if (!value || typeof value !== 'object') return false;
  const r=value as LayoutResult;
  const point=(p: unknown): p is {x:number;y:number} => !!p && typeof p==='object' && Number.isFinite((p as {x:number}).x) && Number.isFinite((p as {y:number}).y);
  if (!Number.isFinite(r.width)||!Number.isFinite(r.height)||r.width<0||r.height<0||!r.positions||!r.routes||!Array.isArray(r.groups)) return false;
  const keys=(record:object, ids:string[])=>Object.keys(record).length===ids.length&&ids.every(id=>Object.hasOwn(record,id));
  if(!keys(r.positions,input.nodes.map(n=>n.id))||!keys(r.routes,input.edges.map(e=>e.id))||r.groups.length!==input.groups.length) return false;
  if(input.nodes.some(n=>!point(r.positions[n.id]))) return false;
  const groups=new Map(r.groups.map(g=>[g.id,g]));
  if(groups.size!==input.groups.length) return false;
  for(const g of input.groups){
    const b=groups.get(g.id);
    if(!b||!point(b)||!Number.isFinite(b.width)||!Number.isFinite(b.height)||b.width<=0||b.height<=0) return false;
    if(g.nodeIds.some(id=>{const p=r.positions[id];return !p||p.x<b.x||p.y<b.y||p.x+input.nodeWidth>b.x+b.width||p.y+input.nodeHeight>b.y+b.height;})) return false;
  }
  const boundary=(p:{x:number;y:number}, n:{x:number;y:number})=>{
    const eps=.01, x=p.x-n.x, y=p.y-n.y;
    return x>=-eps&&y>=-eps&&x<=input.nodeWidth+eps&&y<=input.nodeHeight+eps&&(Math.abs(x)<eps||Math.abs(y)<eps||Math.abs(x-input.nodeWidth)<eps||Math.abs(y-input.nodeHeight)<eps);
  };
  return input.edges.every(e=>{
    const route=r.routes[e.id];
    return Array.isArray(route)&&route.length>=2&&route.every(point)&&boundary(route[0],r.positions[e.source])&&boundary(route[route.length-1],r.positions[e.target]);
  });
}
/** One disposable worker per geometry revision. No SQL/Profile or display text crosses this boundary. */
export function startLayoutTask(input: LayoutInput, done: (result: LayoutResult)=>void, failed:(code:string)=>void,
  createWorker: ()=>LayoutWorker = ()=>new Worker(new URL('./graph-layout.worker.ts',import.meta.url),{type:'module'}), timeoutMs=6000): ()=>void {
  if(input.nodes.length>500||input.edges.length>1000||input.groups.length>500) { failed('layout_budget_exceeded'); return ()=>undefined; }
  let worker: LayoutWorker;
  try { worker=createWorker(); } catch { failed('layout_worker_unavailable'); return ()=>undefined; }
  const id=++sequence;
  let active=true;
  const stop=()=>{if(!active)return;active=false;clearTimeout(timer);worker.onmessage=null;worker.onerror=null;worker.terminate();};
  const timer=setTimeout(()=>{if(active){stop();failed('layout_timeout');}},timeoutMs);
  worker.onmessage=event=>{
    if(!active||!event.data||event.data.id!==id) return;
    stop();
    if(event.data.result) {
      let valid=false;
      try { valid=validResult(input,event.data.result); } catch { /* Fixed diagnostic, never worker content. */ }
      if(valid) done(event.data.result); else failed('layout_invalid_geometry');
    }
    else failed(['layout_invalid_request','layout_invalid_input','layout_budget_exceeded','layout_elk_failed','layout_elk_type_error','layout_elk_reference_error','layout_elk_error','layout_route_direction','layout_missing_route','layout_missing_node','layout_missing_group','layout_invalid_geometry'].includes(event.data.error??'') ? event.data.error! : 'layout_failed');
  };
  worker.onerror=()=>{if(active){stop();failed('layout_worker_failed');}};
  try { worker.postMessage({id,input}); } catch { stop();failed('layout_worker_failed'); }
  return stop;
}
