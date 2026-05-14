import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { describe, expect, it } from "vitest";

import { LoginPage } from "@/pages/LoginPage";
import { AuthProvider } from "@/lib/auth";

function renderLogin() {
  return render(
    <AuthProvider>
      <MemoryRouter initialEntries={["/login"]}>
        <LoginPage />
      </MemoryRouter>
    </AuthProvider>,
  );
}

describe("<LoginPage />", () => {
  it("renders email and password fields", () => {
    renderLogin();
    expect(screen.getByLabelText(/email/i)).toBeInTheDocument();
    expect(screen.getByLabelText(/password/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /sign in/i })).toBeInTheDocument();
  });

  it("links to the register page", () => {
    renderLogin();
    const link = screen.getByRole("link", { name: /create one/i });
    expect(link).toHaveAttribute("href", "/register");
  });

  it("requires both fields before native submit", async () => {
    const user = userEvent.setup();
    renderLogin();
    const email = screen.getByLabelText(/email/i) as HTMLInputElement;
    expect(email.required).toBe(true);
    await user.type(email, "alice@example.com");
    expect(email.value).toBe("alice@example.com");
  });
});
