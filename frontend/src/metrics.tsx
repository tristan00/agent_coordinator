import React from "react";
import { createRoot, type Root } from "react-dom/client";
import { NumberTicker } from "./components/magicui/number-ticker";
import { AnimatedCircularProgressBar } from "./components/magicui/animated-circular-progress-bar";

type Metrics = {tasks:number;attention:number;active:number;unknown:number;connected:number;managed:number;capacity:number;done:number;sendbacks:number;attempts:number};
let root: Root | undefined;
const previous = new Map<string, number>();
function Count({name,value}:{name:string;value:number}) {
  const from = previous.get(name) ?? value;
  previous.set(name,value);
  return matchMedia("(prefers-reduced-motion: reduce)").matches ? <span>{value}</span> : <NumberTicker value={value} startValue={from} className="magic-number" aria-label={String(value)} />;
}
function MetricsBoard({m}:{m:Metrics}) {
  return <div className="stats magic-stats">
    <section className="stat"><div className="stat-top">Tasks</div><div className="stat-value"><Count name="tasks" value={m.tasks}/></div><div className="stat-foot">{m.attention} need attention</div></section>
    <section className="stat"><div className="stat-top">Agents working</div><div className="stat-value">{m.connected?<Count name="active" value={m.active}/>:"—"}{m.unknown>0 && <span> + {m.unknown} unknown</span>}</div><div className="stat-foot">{m.connected?`${m.managed} / ${m.capacity} launch slots`:"No connected agents"}</div></section>
    <section className="stat completion-stat"><div><div className="stat-top">Completed</div><div className="stat-value"><Count name="done" value={m.done}/></div><div className="stat-foot">of {m.tasks} tasks</div></div><div role="progressbar" aria-label="Task completion" aria-valuenow={m.done} aria-valuemax={Math.max(1,m.tasks)} aria-valuemin={0}><AnimatedCircularProgressBar value={m.done} max={Math.max(1,m.tasks)} gaugePrimaryColor="var(--purple)" gaugeSecondaryColor="var(--line)" className="magic-progress"/></div></section>
    <section className="stat"><div className="stat-top">Sendbacks</div><div className="stat-value"><Count name="sendbacks" value={m.sendbacks}/></div><div className="stat-foot">{m.attempts} attempts</div></section>
  </div>;
}
export function unmountMetrics() {root?.unmount();root=undefined;}
export function mountMetrics(element:HTMLElement|null,m:Metrics) {if(element){root=createRoot(element);root.render(<MetricsBoard m={m}/>);}}
