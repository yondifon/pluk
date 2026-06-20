import { test, expect, describe } from "bun:test";
import { evaluateCommand, sanitizeWorkingDir } from "./policy.js";

describe("ssh command policy", () => {
  test("allows safe read commands", () => {
    for (const cmd of ["ls -la", "df -h", "uptime", "cat /var/log/app.log", "docker ps", "docker compose ps", "git status", "ps aux | grep node"]) {
      const v = evaluateCommand(cmd);
      expect(v.ok).toBe(true);
      expect(v.category).toBe("read");
    }
  });

  test("classifies compose lifecycle as write", () => {
    expect(evaluateCommand("docker compose up -d")).toMatchObject({ ok: true, category: "write" });
    expect(evaluateCommand("docker compose restart web")).toMatchObject({ ok: true, category: "write" });
  });

  test("blocks shell chaining, redirection, and substitution", () => {
    for (const cmd of [
      "ls; rm -rf /",
      "ls && rm file",
      "ls || whoami",
      "cat a > b",
      "cat a >> b",
      "cat < file",
      "echo $(whoami)",
      "echo ${HOME}",
      "ls `whoami`",
      "ls & whoami",
    ]) {
      expect(evaluateCommand(cmd).ok).toBe(false);
    }
  });

  test("blocks non-allowlisted binaries", () => {
    for (const cmd of ["rm -rf /tmp", "curl http://evil", "wget x", "bash -c 'id'", "python -c 'x'", "env", "printenv", "scp a b"]) {
      expect(evaluateCommand(cmd).ok).toBe(false);
    }
  });

  test("blocks destructive docker/compose/systemctl/git subcommands", () => {
    for (const cmd of [
      "docker exec -it web sh",
      "docker run alpine",
      "docker rm web",
      "docker system prune",
      "docker compose down",
      "docker compose rm",
      "systemctl restart nginx",
      "git push",
      "git checkout main",
      "kubectl delete pod x",
      "kubectl exec pod -- sh",
    ]) {
      expect(evaluateCommand(cmd).ok).toBe(false);
    }
  });

  test("blocks reading sensitive files", () => {
    for (const cmd of [
      "cat .env",
      "cat /srv/app/.env.production",
      "tail ~/.ssh/id_rsa",
      "grep secret ~/.aws/credentials",
      "cat server.pem",
      "less /etc/shadow",
    ]) {
      expect(evaluateCommand(cmd).ok).toBe(false);
    }
  });

  test("blocks find -exec / -delete escapes", () => {
    expect(evaluateCommand("find . -name '*.log' -delete").ok).toBe(false);
    expect(evaluateCommand("find / -exec rm {} ;").ok).toBe(false);
    expect(evaluateCommand("find /var/log -name '*.log'").ok).toBe(true);
  });

  test("every pipeline segment must pass", () => {
    expect(evaluateCommand("docker ps | grep web").ok).toBe(true);
    expect(evaluateCommand("ls | rm -rf x").ok).toBe(false);
    expect(evaluateCommand("cat foo | bash").ok).toBe(false);
  });

  test("blocks brace-comma expansion but allows docker --format templates", () => {
    expect(evaluateCommand("cat {.env,foo}").ok).toBe(false);
    expect(evaluateCommand("cat /etc/{passwd,shadow}").ok).toBe(false);
    expect(evaluateCommand("docker inspect --format '{{.State.Running}}' web").ok).toBe(true);
  });

  test("blocks tail/journalctl follow (would hang)", () => {
    expect(evaluateCommand("tail -f /var/log/app.log").ok).toBe(false);
    expect(evaluateCommand("journalctl -u nginx -f").ok).toBe(false);
    expect(evaluateCommand("tail -n 100 /var/log/app.log").ok).toBe(true);
    expect(evaluateCommand("journalctl -u nginx -n 50").ok).toBe(true);
  });

  test("sanitizeWorkingDir rejects unsafe paths", () => {
    expect(sanitizeWorkingDir("/srv/app")).toBe("/srv/app");
    expect(sanitizeWorkingDir("~/project")).toBe("~/project");
    expect(sanitizeWorkingDir("/srv/$(whoami)")).toBeNull();
    expect(sanitizeWorkingDir("/srv; rm -rf /")).toBeNull();
    expect(sanitizeWorkingDir("/srv/app/.env")).toBeNull();
    expect(sanitizeWorkingDir("/path with space")).toBeNull();
  });
});
