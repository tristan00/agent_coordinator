import React from 'react';

export const matchesTask = (task, query) => {
  const needle = query.toLowerCase();
  return task.title.toLowerCase().includes(needle) || (task.shortId || '').toLowerCase().includes(needle);
};

export const taskLabel = task => task.shortId ? `${task.shortId} ${task.title}` : task.title;

export const modelText = task => (task.models || []).map(m => m.effort ? `${m.model} · ${m.effort}` : m.model).join(', ');

export default function TaskName({task}) {
  return <>{task.shortId && <span className="task-id">{task.shortId}</span>}{task.title}</>;
}
