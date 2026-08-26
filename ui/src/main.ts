import "./style.css";

const app = document.querySelector<HTMLDivElement>("#app");

if (app) {
  app.innerHTML = `
    <h1>Pluk</h1>
    <p>Expose your services to AI agents as MCP tools.</p>
  `;
}
