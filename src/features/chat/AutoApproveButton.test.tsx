import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { AutoApproveButton } from "./AutoApproveButton";
import { useApp } from "@/app/store";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("AutoApproveButton", () => {
  it("warns before enabling for the current chat", async () => {
    const setAutoApprove = vi.fn(async (enabled: boolean) => {
      useApp.setState({ autoApprove: enabled });
    });
    useApp.setState({ autoApprove: false, setAutoApprove });
    render(<AutoApproveButton />);

    fireEvent.click(screen.getByRole("button", { name: /Auto/ }));
    expect(screen.getByText("Enable auto-approve?")).toBeInTheDocument();
    expect(screen.getByText(/dangerous shell commands/i)).toBeInTheDocument();
    expect(screen.getByText(/outside-workspace writes/i)).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Enable for this session" }));
    await waitFor(() => expect(setAutoApprove).toHaveBeenCalledWith(true));
    await waitFor(() => expect(screen.queryByText("Enable auto-approve?")).not.toBeInTheDocument());
  });
});
