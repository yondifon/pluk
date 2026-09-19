import { describe, test, expect } from "bun:test";
import { logoType } from "./adapterLogo";

describe("MCP servers borrow the logo of the brand hosting them", () => {
  test("a brand in the host wins over the lettered glyph", () => {
    expect(logoType("mcp", "https://mcp.linear.app/sse")).toBe("linear");
    expect(logoType("mcp", "https://mcp.sentry.dev/mcp")).toBe("sentry");
    expect(logoType("mcp", "https://slack.com/api/mcp")).toBe("slack");
  });

  test("an unknown host keeps the lettered glyph", () => {
    expect(logoType("mcp", "https://mcp.example.com/sse")).toBe("mcp");
  });

  test("a brand name outside the domain does not borrow the logo", () => {
    expect(logoType("mcp", "https://linear.evil.com/sse")).toBe("mcp");
    expect(logoType("mcp", "https://sentry.example.com.evil/sse")).toBe("mcp");
  });

  test("only MCP integrations borrow a brand", () => {
    expect(logoType("wande", "https://mcp.linear.app/sse")).toBe("wande");
    expect(logoType("postgres")).toBe("postgres");
  });

  test("a missing or unparseable URL keeps the lettered glyph", () => {
    expect(logoType("mcp")).toBe("mcp");
    expect(logoType("mcp", "")).toBe("mcp");
    expect(logoType("mcp", "mcp.linear.app")).toBe("mcp");
  });

  test("inherited object properties are not brands", () => {
    expect(logoType("mcp", "https://mcp.constructor.com/sse")).toBe("mcp");
  });
});
