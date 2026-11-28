import { AddProviderDialog } from "@/components/providers/AddProviderDialog";
import { useAddProviderMutation } from "@/lib/query/mutations";
import type { Provider } from "@/types";

export default function ProfileAddProvider({
  onClose,
  onAdded,
}: {
  onClose: () => void;
  onAdded: (provider: Provider) => void;
}) {
  const add = useAddProviderMutation("claude");
  return (
    <AddProviderDialog
      open
      appId="claude"
      modeView="direct"
      allowUniversal={false}
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
      onSubmit={async (input) => {
        const provider = await add.mutateAsync({
          ...input,
          addToLive: false,
          editorSave: { base: {}, connectionOnly: true },
        });
        onAdded(provider);
      }}
    />
  );
}
