const content = document.querySelector("#content");
const status = document.querySelector("#status");
const pagination = document.querySelector("#pagination");
const previous = document.querySelector("#previous");
const next = document.querySelector("#next");
const pageLabel = document.querySelector("#page-label");
const pageSize = 100;
let current = { view: "summary" };
let offset = 0;
let total = 0;
let previousViews = [];

function node(tag, value, className) {
  const element = document.createElement(tag);
  if (value !== undefined && value !== null) element.textContent = String(value);
  if (className) element.className = className;
  return element;
}

function buttonLink(label, query) {
  const button = node("button", label, "action");
  button.type = "button";
  button.addEventListener("click", () => navigate(query, true));
  return button;
}

function cell(value) {
  const element = node("td");
  if (value instanceof Node) element.append(value);
  else element.textContent = value === null || value === undefined || value === "" ? "—" : String(value);
  return element;
}

function table(headers, rows) {
  const wrapper = node("div", undefined, "panel");
  const result = node("table");
  const head = node("thead");
  const headerRow = node("tr");
  for (const label of headers) headerRow.append(node("th", label));
  head.append(headerRow);
  result.append(head);
  const body = node("tbody");
  for (const values of rows) {
    const row = node("tr");
    for (const value of values) row.append(cell(value));
    body.append(row);
  }
  result.append(body);
  wrapper.append(result);
  return wrapper;
}

function heading(title, level = 1) {
  return node(`h${level}`, title);
}

function cards(items) {
  const wrapper = node("div", undefined, "cards");
  for (const [label, value] of items) {
    const card = node("div", undefined, "card");
    card.append(node("span", label, "label"), node("strong", value));
    wrapper.append(card);
  }
  return wrapper;
}

function date(value) {
  if (value === null || value === undefined) return "—";
  if (/^\d+$/.test(String(value))) {
    const parsed = new Date(Number(value));
    if (!Number.isNaN(parsed.valueOf())) return parsed.toLocaleString();
  }
  return String(value);
}

function duration(value) {
  if (value === null || value === undefined) return "Unknown";
  if (value < 1000) return `${value} ms`;
  const seconds = value / 1000;
  if (seconds < 60) return `${seconds.toFixed(1)} s`;
  return `${Math.floor(seconds / 60)}m ${Math.floor(seconds % 60)}s`;
}

function sourceLinks(ids) {
  if (!ids?.length) return "—";
  const wrapper = node("span");
  ids.forEach((id, index) => {
    if (index) wrapper.append(document.createTextNode(", "));
    wrapper.append(buttonLink(`#${id}`, { view: "source_event", id }));
  });
  return wrapper;
}

function evidenceLink(item) {
  const kinds = {
    run: "run",
    tool_call: "tool_call",
    skill_call: "skill_call",
    skill_inventory: "skill_inventory",
  };
  if (!kinds[item.entity_type]) return `${item.entity_type} #${item.id}`;
  return buttonLink(`${item.entity_type} #${item.id}`, { view: "evidence", kind: item.entity_type, id: item.id });
}

function evidenceList(items) {
  if (!items?.length) return "—";
  const wrapper = node("span");
  items.forEach((item, index) => {
    if (index) wrapper.append(document.createTextNode(", "));
    wrapper.append(evidenceLink(item));
  });
  return wrapper;
}

function runRows(items) {
  return items.map((run) => [
    buttonLink(`#${run.id}`, { view: "run", id: run.id }),
    run.task_id === null ? "Unlinked" : buttonLink(run.task_title || `Task #${run.task_id}`, { view: "task", id: run.task_id }),
    run.outcome,
    date(run.started_at),
    duration(run.duration_ms),
    `${run.tool_calls} tools · ${run.skill_calls} skills`,
    sourceLinks(run.evidence),
  ]);
}

function taskRows(items) {
  return items.map((task) => [
    buttonLink(task.title || `Task #${task.id}`, { view: "task", id: task.id }),
    task.source,
    task.source_key,
    task.runs,
  ]);
}

function skillRows(items) {
  return items.map((skill) => [
    buttonLink(skill.name, { view: "skill_calls", skill: skill.name }),
    skill.status,
    skill.installed ? skill.providers.join(", ") : "Observed only",
    skill.calls,
    skill.runs,
    date(skill.last_used),
  ]);
}

function toolRows(items) {
  return items.map((tool) => [
    buttonLink(tool.name, { view: "tool", name: tool.name }),
    tool.calls,
    tool.failed_calls,
    tool.runs,
  ]);
}

function appendDetail(title, values) {
  content.append(heading(title));
  const grid = node("div", undefined, "detail-grid");
  for (const [label, value] of values) {
    const panel = node("div", undefined, "panel");
    const valueElement = node("div");
    if (value instanceof Node) valueElement.append(value);
    else valueElement.textContent = value === null || value === undefined || value === "" ? "—" : String(value);
    panel.append(node("span", label, "label"), valueElement);
    grid.append(panel);
  }
  content.append(grid);
}
function backButton() {
  const button = node("button", "← Previous view", "action");
  button.type = "button";
  button.addEventListener("click", goBack);
  return button;
}


function renderSummary(data) {
  const summary = data.summary;
  content.append(heading("Overview"));
  content.append(cards([
    ["Runs", summary.runs],
    ["Tasks", summary.tasks],
    ["Skills", summary.skills],
    ["Tool calls", summary.tool_calls],
    ["Skill calls", summary.skill_calls],
    ["Stored findings", summary.findings],
    ["Computed waste signals", summary.computed_findings],
  ]));
  content.append(heading("Skill activity", 2));
  content.append(table(["Active", "Dormant", "Never used", "Unknown"], [[
    summary.skill_status.active,
    summary.skill_status.dormant,
    summary.skill_status.never_used,
    summary.skill_status.unknown,
  ]]));
  content.append(heading("Run durations", 2));
  const bins = summary.durations;
  content.append(table(["< 1 min", "1–5 min", "5–15 min", "15–60 min", "> 60 min", "Unknown"], [[
    bins.under_one_minute,
    bins.one_to_five_minutes,
    bins.five_to_fifteen_minutes,
    bins.fifteen_to_sixty_minutes,
    bins.over_sixty_minutes,
    bins.unknown,
  ]]));
  content.append(heading("Tasks", 2));
  content.append(table(["Task", "Source", "Source key", "Runs"], taskRows(data.tasks)));
  content.append(heading("Runs", 2));
  content.append(table(["Run", "Task", "Outcome", "Started", "Duration", "Calls", "Source events"], runRows(data.runs)));
}

function renderTasks(items) {
  content.append(heading("Tasks"));
  content.append(table(["Task", "Source", "Source key", "Runs"], taskRows(items)));
}

function renderTask(detail) {
  content.append(buttonLink("← Tasks", { view: "tasks" }));
  appendDetail(detail.task.title || `Task #${detail.task.id}`, [
    ["Source", detail.task.source],
    ["Source key", detail.task.source_key],
    ["Runs", detail.task.runs],
  ]);
  content.append(heading("Runs", 2));
  content.append(table(["Run", "Task", "Outcome", "Started", "Duration", "Calls", "Source events"], runRows(detail.runs)));
}

function renderRuns(items) {
  content.append(heading("Runs"));
  content.append(table(["Run", "Task", "Outcome", "Started", "Duration", "Calls", "Source events"], runRows(items)));
}

function renderRun(detail) {
  const run = detail.run;
  content.append(buttonLink("← Runs", { view: "runs" }));
  appendDetail(`Run #${run.id}`, [
    ["Task", run.task_id === null ? "Unlinked" : buttonLink(run.task_title || `Task #${run.task_id}`, { view: "task", id: run.task_id })],
    ["Outcome", run.outcome],
    ["Started", date(run.started_at)],
    ["Ended", date(run.ended_at)],
    ["Duration", duration(run.duration_ms)],
    ["Agent", run.agent],
    ["Model", run.model],
    ["Sessions", run.sessions.join(", ")],
    ["Source events", sourceLinks(run.evidence)],
  ]);
  content.append(heading(`Tool calls (${run.tool_calls})`, 2));
  content.append(table(["Tool", "Status", "Started", "Ended", "Source events"], detail.tool_calls.map((call) => [
    buttonLink(`${call.tool} #${call.id}`, { view: "evidence", kind: "tool_call", id: call.id }),
    call.status,
    date(call.started_at),
    date(call.ended_at),
    sourceLinks(call.evidence),
  ])));
  content.append(heading(`Skill calls (${run.skill_calls})`, 2));
  content.append(table(["Skill", "Provider", "Started", "Source events"], detail.skill_calls.map((call) => [
    buttonLink(`${call.skill} #${call.id}`, { view: "evidence", kind: "skill_call", id: call.id }),
    call.provider,
    date(call.started_at),
    sourceLinks(call.evidence),
  ])));
  if (detail.tool_calls.length < run.tool_calls || detail.skill_calls.length < run.skill_calls) {
    const remaining = Math.max(run.tool_calls, run.skill_calls);
    content.append(node("p", `Showing a page of related calls. ${remaining} calls are available for this run.` , "muted"));
  }
}

function renderSkills(data) {
  content.append(heading("Skills"));
  content.append(node("p", "Active means a valid call was recorded within the inactivity window. Dormant means all observed timestamps are valid and outside it. Never used means an installed skill has no calls. Unknown means the evidence cannot establish recency.", "muted"));
  content.append(table(["Skill", "Status", "Providers", "Calls", "Runs", "Last observed"], skillRows(data.items)));
  content.append(heading("Run × skill calls", 2));
  content.append(table(["Run", "Skill", "Calls"], data.matrix.map((cell) => [
    buttonLink(`#${cell.run_id}`, { view: "run", id: cell.run_id }),
    buttonLink(cell.skill, { view: "skill_calls", run_id: cell.run_id, skill: cell.skill }),
    cell.calls,
  ])));
}

function renderSkillCalls(items, query) {
  const label = query.run_id !== undefined
    ? `Skill calls · ${query.skill} · Run #${query.run_id}`
    : `Skill calls · ${query.skill}`;
  content.append(buttonLink("← Skills", { view: "skills" }));
  content.append(heading(label));
  content.append(table(["Skill", "Run", "Provider", "Started", "Source events"], items.map((call) => [
    call.skill,
    call.run_id === null ? "Unlinked" : buttonLink(`#${call.run_id}`, { view: "run", id: call.run_id }),
    call.provider,
    date(call.started_at),
    sourceLinks(call.evidence),
  ])));
}

function renderTools(items) {
  content.append(heading("Tools"));
  content.append(table(["Tool", "Calls", "Failed calls", "Runs"], toolRows(items)));
}

function renderToolCalls(name, items) {
  content.append(buttonLink("← Tools", { view: "tools" }));
  content.append(heading(`Tool · ${name}`));
  content.append(table(["Call", "Run", "Status", "Started", "Ended", "Source events"], items.map((call) => [
    buttonLink(`#${call.id}`, { view: "evidence", kind: "tool_call", id: call.id }),
    call.run_id === null ? "Unlinked" : buttonLink(`#${call.run_id}`, { view: "run", id: call.run_id }),
    call.status,
    date(call.started_at),
    date(call.ended_at),
    sourceLinks(call.evidence),
  ])));
}

function renderAgents(items, matrix) {
  content.append(heading("Agents & models"));
  content.append(node("p", "Attribution is grouped only from agent and model values stored on normalized runs. Unattributed runs are not assigned to a guessed provider.", "muted"));
  content.append(table(["Agent", "Model", "Runs"], items.map((item) => [item.agent, item.model, item.runs])));
  content.append(heading("Skill calls by agent and model", 2));
  content.append(table(["Agent", "Model", "Skill", "Calls"], matrix.map((item) => [item.agent, item.model, item.skill, item.calls])));
}

function renderWaste(items) {
  content.append(heading("Waste signals"));
  content.append(node("p", "Evidence-backed indicators, not claims that work was unnecessary. Repeated hashes and long durations do not prove waste.", "muted"));
  content.append(table(["Signal", "Explanation", "Metric", "Threshold", "Source evidence"], items.map((finding) => [
    finding.finding_type,
    finding.explanation,
    `${finding.metric.value} ${finding.metric.unit}`,
    finding.threshold ? `${finding.threshold.value} ${finding.threshold.unit}` : "—",
    evidenceList(finding.evidence),
  ])));
}

function renderEvidence(detail) {
  content.append(backButton());
  const record = detail.record.record;
  const type = detail.record.type.replaceAll("_", " ");
  content.append(heading(`${type} #${record.id}`));
  const values = Object.entries(record).filter(([key]) => key !== "evidence").map(([key, value]) => [key.replaceAll("_", " "), Array.isArray(value) ? value.join(", ") : value]);
  appendDetail("Normalized record", values);
  content.append(heading("Source events", 2));
  content.append(table(["Event", "Source", "Identity", "Observed", "Source timestamp"], detail.source_events.map((event) => [
    buttonLink(`#${event.id}`, { view: "source_event", id: event.id }),
    `${event.source_type} · ${event.source_path}`,
    event.source_identity,
    date(event.observed_at),
    date(event.source_timestamp),
  ])));
}

function renderSourceEvent(event) {
  content.append(backButton());
  appendDetail(`Source event #${event.id}`, [
    ["Source", `${event.source_type} · ${event.source_path}`],
    ["Identity", event.source_identity],
    ["Observed", date(event.observed_at)],
    ["Source timestamp", date(event.source_timestamp)],
    ["Payload hash", event.payload_hash],
    ["Payload bytes", event.payload_bytes],
  ]);
  if (event.payload_truncated) {
    content.append(node("p", `Showing ${event.payload.length.toLocaleString()} characters of the ${event.payload_bytes.toLocaleString()}-byte stored event.`, "muted"));
  }
  content.append(heading("Payload preview", 2));
  content.append(node("pre", event.payload));

}
function render(query, data) {
  content.replaceChildren();
  switch (query.view) {
    case "summary": renderSummary(data); break;
    case "tasks": renderTasks(data); break;
    case "task": renderTask(data); break;
    case "runs": renderRuns(data.items); break;
    case "run": renderRun(data); break;
    case "skills": renderSkills(data); break;
    case "skill_calls": renderSkillCalls(data, query); break;
    case "tools": renderTools(data); break;
    case "tool": renderToolCalls(query.name, data); break;
    case "agent_models": renderAgents(data.models, data.skill_matrix); break;
    case "waste": renderWaste(data.items); break;
    case "evidence": renderEvidence(data); break;
    case "source_event": renderSourceEvent(data); break;
    default: content.append(node("p", "Unsupported report view."));
  }
}

function effectiveTotal(query, data, metadata) {
  if (query.view === "run") return Math.max(1, data.run.tool_calls, data.run.skill_calls);
  if (query.view === "task") return data.task.runs;
  return metadata.total;
}

async function load(query, pageOffset = 0) {
  current = query;
  offset = pageOffset;
  status.classList.remove("error");
  status.textContent = "Loading evidence…";
  const params = new URLSearchParams({ ...query, limit: String(pageSize), offset: String(offset) });
  try {
    const response = await fetch(`/api/report?${params}`, { cache: "no-store" });
    if (!response.ok) throw new Error(`Report request failed (${response.status})`);
    let snapshot = await response.json();
    if (query.view === "summary") {
      const related = async (view) => {
        const relatedParams = new URLSearchParams({ view, limit: "5", offset: "0" });
        const relatedResponse = await fetch(`/api/report?${relatedParams}`, { cache: "no-store" });
        if (!relatedResponse.ok) throw new Error(`${view} report request failed (${relatedResponse.status})`);
        return relatedResponse.json();
      };
      const [tasks, runs] = await Promise.all([related("tasks"), related("runs")]);
      snapshot = {
        view: { summary: snapshot.view, tasks: tasks.view, runs: runs.view.items },
        metadata: snapshot.metadata,
      };
    } else if (query.view === "skills") {
      const matrixParams = new URLSearchParams({ view: "skill_matrix", limit: String(pageSize), offset: String(offset) });
      const matrixResponse = await fetch(`/api/report?${matrixParams}`, { cache: "no-store" });
      if (!matrixResponse.ok) throw new Error(`Skill matrix request failed (${matrixResponse.status})`);
      const matrixSnapshot = await matrixResponse.json();
      snapshot = {
        view: { items: snapshot.view.items, matrix: matrixSnapshot.view },
        metadata: { ...snapshot.metadata, total: Math.max(snapshot.metadata.total, matrixSnapshot.metadata.total) },
      };
    } else if (query.view === "agent_models") {
      const matrixParams = new URLSearchParams({ view: "agent_skill_matrix", limit: String(pageSize), offset: String(offset) });
      const matrixResponse = await fetch(`/api/report?${matrixParams}`, { cache: "no-store" });
      if (!matrixResponse.ok) throw new Error(`Skill attribution request failed (${matrixResponse.status})`);
      const matrixSnapshot = await matrixResponse.json();
      snapshot = {
        view: { models: snapshot.view, skill_matrix: matrixSnapshot.view },
        metadata: { ...snapshot.metadata, total: Math.max(snapshot.metadata.total, matrixSnapshot.metadata.total) },
      };
    }
    total = effectiveTotal(query, snapshot.view, snapshot.metadata);
    render(query, snapshot.view);
    status.textContent = `${total.toLocaleString()} matching records`;
    const hasPages = total > pageSize;
    pagination.hidden = !hasPages;
    previous.disabled = offset === 0;
    next.disabled = offset + pageSize >= total;
    pageLabel.textContent = `${offset + 1}–${Math.min(offset + pageSize, total)} of ${total}`;
  } catch (error) {
    content.replaceChildren(node("p", error.message));
    status.classList.add("error");
    status.textContent = "Could not load local report data.";
    pagination.hidden = true;
  }
}

function navigate(query, keepCurrent) {
  if (keepCurrent && current) previousViews.push(current);
  document.querySelectorAll("nav [data-view]").forEach((button) => button.classList.toggle("active", button.dataset.view === query.view));
  load(query, 0);
}

function goBack() {
  navigate(previousViews.pop() || { view: "summary" }, false);
}

document.querySelectorAll("nav [data-view]").forEach((button) => {
  button.addEventListener("click", () => navigate({ view: button.dataset.view }, false));
});
previous.addEventListener("click", () => load(current, Math.max(0, offset - pageSize)));
next.addEventListener("click", () => load(current, offset + pageSize));
navigate({ view: "summary" }, false);
