import {afterEach,describe,expect,it,vi} from 'vitest';
import {startLayoutTask} from './graph-layout-client';
import type {LayoutInput,LayoutResult} from './graph-layout';

const input:LayoutInput={nodes:[{id:'one'}],edges:[],groups:[],nodeWidth:224,nodeHeight:74};
const result:LayoutResult={positions:{one:{x:0,y:0}},groups:[],routes:{},width:224,height:74};
function setup(timeout=6000){
  const worker={onmessage:null as ((event:MessageEvent)=>void)|null,onerror:null as ((event:ErrorEvent)=>void)|null,postMessage:vi.fn(),terminate:vi.fn()};
  const done=vi.fn(),failed=vi.fn();
  const stop=startLayoutTask(input,done,failed,()=>worker,timeout);
  return {worker,done,failed,stop,id:worker.postMessage.mock.calls[0][0].id};
}
afterEach(()=>vi.useRealTimers());
describe('bounded disposable geometry worker',()=>{
  it('rejects truncated, nonfinite, missing-route and reversed-route results',()=>{
    const fixture:LayoutInput={nodes:[{id:'a'},{id:'b'}],edges:[{id:'ab',source:'a',target:'b',reverseForLayout:false}],groups:[{id:'g',nodeIds:['a','b']}],nodeWidth:224,nodeHeight:74};
    const valid:LayoutResult={positions:{a:{x:0,y:0},b:{x:0,y:100}},groups:[{id:'g',x:0,y:0,width:224,height:174}],routes:{ab:[{x:112,y:74},{x:112,y:100}]},width:224,height:174};
    for(const mutate of [(r:LayoutResult)=>{delete r.positions.a;},(r:LayoutResult)=>{r.positions.a.x=NaN;},(r:LayoutResult)=>{delete r.routes.ab;},(r:LayoutResult)=>{r.routes.ab.reverse();},(r:LayoutResult)=>{r.groups=[];}]){
      const worker={onmessage:null as ((event:MessageEvent)=>void)|null,onerror:null as ((event:ErrorEvent)=>void)|null,postMessage:vi.fn(),terminate:vi.fn()};
      const done=vi.fn(),fail=vi.fn();startLayoutTask(fixture,done,fail,()=>worker);const broken=structuredClone(valid);mutate(broken);
      worker.onmessage?.({data:{id:worker.postMessage.mock.calls[0][0].id,result:broken}} as MessageEvent);
      expect(done).not.toHaveBeenCalled();expect(fail).toHaveBeenCalledWith('layout_invalid_geometry');expect(worker.terminate).toHaveBeenCalledOnce();
    }
  });
  it('handles error/post failures and makes cleanup idempotent',()=>{
    const t=setup();t.worker.onerror?.({} as ErrorEvent);t.stop();t.stop();expect(t.failed).toHaveBeenCalledWith('layout_worker_failed');expect(t.worker.terminate).toHaveBeenCalledOnce();
    const fail=vi.fn(),terminate=vi.fn();startLayoutTask(input,vi.fn(),fail,()=>({onmessage:null,onerror:null,terminate,postMessage:()=>{throw Error('private path');}}));expect(fail).toHaveBeenCalledWith('layout_worker_failed');expect(terminate).toHaveBeenCalledOnce();
  });
  it('sends geometry only and accepts only its task id',()=>{const t=setup();expect(t.worker.postMessage.mock.calls[0][0].input).toEqual(input);t.worker.onmessage?.({data:{id:t.id-1,result}} as MessageEvent);expect(t.done).not.toHaveBeenCalled();t.worker.onmessage?.({data:{id:t.id,result}} as MessageEvent);expect(t.done).toHaveBeenCalledWith(result);expect(t.worker.terminate).toHaveBeenCalledOnce();});
  it('ignores late responses after report replacement/unmount',()=>{const t=setup();const late=t.worker.onmessage;t.stop();late?.({data:{id:t.id,result}} as MessageEvent);expect(t.done).not.toHaveBeenCalled();expect(t.failed).not.toHaveBeenCalled();});
  it('terminates timeout and can start a fresh worker',()=>{vi.useFakeTimers();const t=setup(50);vi.advanceTimersByTime(50);expect(t.failed).toHaveBeenCalledWith('layout_timeout');expect(t.worker.terminate).toHaveBeenCalledOnce();const retry=setup();retry.worker.onmessage?.({data:{id:retry.id,result}} as MessageEvent);expect(retry.done).toHaveBeenCalledOnce();});
  it('handles unavailable workers and enforces budgets before posting',()=>{const fail=vi.fn(),factory=vi.fn(()=>{throw Error('private path');});startLayoutTask(input,vi.fn(),fail,factory);expect(fail).toHaveBeenCalledWith('layout_worker_unavailable');factory.mockClear();startLayoutTask({...input,nodes:Array.from({length:501},(_,i)=>({id:String(i)}))},vi.fn(),fail,factory);expect(factory).not.toHaveBeenCalled();expect(fail).toHaveBeenCalledWith('layout_budget_exceeded');});
});
