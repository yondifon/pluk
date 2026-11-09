// The window shown while an SSH connect waits on the key agent. It only
// explains the wait: the approval itself happens in the agent's own prompt.
const params = new URLSearchParams(window.location.search);
const serverName = params.get("name") || "this server";
const agent = params.get("agent");

const title = document.getElementById("signin-title");
const hint = document.getElementById("signin-hint");
if (title) {
  title.textContent = `Connecting to ${serverName}`;
}
if (hint) {
  hint.textContent = agent
    ? `Approve the sign-in in ${agent} if it asks. This window closes on its own.`
    : "Approve the sign-in in your key agent if it asks. This window closes on its own.";
}

export {};
