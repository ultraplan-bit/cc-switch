import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { ApiKeySection } from "@/components/providers/forms/shared/ApiKeySection";

const { copyText } = vi.hoisted(() => ({
  copyText: vi.fn(() => Promise.resolve()),
}));
vi.mock("@/lib/clipboard", () => ({ copyText }));

const { toast } = vi.hoisted(() => ({
  toast: { success: vi.fn(), error: vi.fn() },
}));
vi.mock("@/lib/toast", () => ({ toast }));

// 让 t() 原样返回 key，以便检查旧的合作伙伴信息不会再显示推广文案。
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

function renderSection(
  props: Partial<React.ComponentProps<typeof ApiKeySection>> = {},
) {
  return render(
    <ApiKeySection
      value=""
      onChange={() => {}}
      category="third_party"
      shouldShowLink
      websiteUrl="https://example.com/keys"
      partnerPromotionKey="kimi"
      {...props}
    />,
  );
}

describe("ApiKeySection", () => {
  it("keeps the API Key link and hint without rendering legacy promotions", () => {
    renderSection({ isPartner: true, hint: "API Key help" });

    expect(
      screen.getByRole("link", { name: /providerForm\.getApiKey/ }),
    ).toHaveAttribute("href", "https://example.com/keys");
    expect(
      screen.queryByText("providerForm.partnerPromotion.kimi"),
    ).not.toBeInTheDocument();
    expect(screen.getByText("API Key help")).toBeInTheDocument();
  });

  it("hides the promotion when the API Key link is hidden", () => {
    renderSection({ category: "official", shouldShowLink: false });

    expect(
      screen.queryByText("providerForm.partnerPromotion.kimi"),
    ).not.toBeInTheDocument();
  });

  it("renders nothing extra for presets without a promotion", () => {
    const { container } = renderSection({ partnerPromotionKey: undefined });

    expect(container.querySelector("p")).toBeNull();
  });

  it("turns off autocorrect and auto-capitalization on the key input", () => {
    const { container } = renderSection({ value: "sk-test" });
    const input = container.querySelector("input");

    expect(input).toHaveAttribute("autocorrect", "off");
    expect(input).toHaveAttribute("autocapitalize", "off");
    expect(input).toHaveAttribute("spellcheck", "false");
  });

  it("copies the whole key from the label row", async () => {
    copyText.mockClear();
    renderSection({ value: "sk-abcdef1234567890" });

    await userEvent.click(screen.getByRole("button", { name: /common\.copy/ }));

    expect(copyText).toHaveBeenCalledWith("sk-abcdef1234567890");
  });

  it("copies the trimmed key, matching what forms save", async () => {
    copyText.mockClear();
    renderSection({ value: "  sk-abcdef1234567890\n" });

    await userEvent.click(screen.getByRole("button", { name: /common\.copy/ }));

    expect(copyText).toHaveBeenCalledWith("sk-abcdef1234567890");
  });

  it("hides the copy button while the key is empty", () => {
    renderSection({ value: "" });

    expect(
      screen.queryByRole("button", { name: /common\.copy/ }),
    ).not.toBeInTheDocument();
  });

  it("shows the failure toast when copying fails", async () => {
    copyText.mockRejectedValueOnce(new Error("clipboard denied"));
    toast.error.mockClear();
    renderSection({ value: "sk-abcdef1234567890" });

    await userEvent.click(screen.getByRole("button", { name: /common\.copy/ }));

    expect(toast.error).toHaveBeenCalledWith("apiKeyInput.copyFailed");
  });

  it("hides the copy button when the input is disabled", () => {
    renderSection({ value: "sk-abcdef1234567890", category: "official" });

    expect(
      screen.queryByRole("button", { name: /common\.copy/ }),
    ).not.toBeInTheDocument();
  });

  it("hides the copy button when the key is whitespace only", () => {
    renderSection({ value: "   " });

    expect(
      screen.queryByRole("button", { name: /common\.copy/ }),
    ).not.toBeInTheDocument();
  });
});
