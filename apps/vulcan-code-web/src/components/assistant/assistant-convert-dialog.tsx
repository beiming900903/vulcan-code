import React from "react";
import { toast } from "@/ui-kit/primitives/toast";
import { dedicatedErrorMessage } from "@/components/composer/dedicated/dedicated-errors";
import { Button } from "@/ui-kit/primitives/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/ui-kit/primitives/dialog";
import { Input } from "@/ui-kit/primitives/input";
import { Spinner } from "@/ui-kit/primitives/spinner";
import { useI18n } from "@/i18n";

/** Names a memory-locked dedicated session and turns it into a floating assistant.
 * 为记忆已锁定的专职会话命名，并将其转为浮动助手。
 *
 * @param props.defaultName prefilled name, normally the session title.
 *                          预填名称，通常为会话标题。
 * @param props.onConvert performs the conversion; its rejection stays in the dialog. */
export function AssistantConvertDialog(props: {
  open: boolean;
  defaultName: string;
  onOpenChange: (open: boolean) => void;
  onConvert: (name: string) => Promise<void>;
}) {
  const { t } = useI18n();
  const [name, setName] = React.useState(props.defaultName);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState<string | null>(null);
  React.useEffect(() => {
    if (!props.open) return;
    setName(props.defaultName);
    setError(null);
  }, [props.defaultName, props.open]);
  const submit = async () => {
    if (busy || !name.trim()) return;
    setBusy(true);
    setError(null);
    try {
      await props.onConvert(name.trim());
      toast.success(t("assistantDock.converted"));
      props.onOpenChange(false);
    } catch (reason) {
      setError(dedicatedErrorMessage(reason, t));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog open={props.open} onOpenChange={(open) => !busy && props.onOpenChange(open)}>
      <DialogContent showCloseButton={!busy} size="sm">
        <DialogHeader>
          <DialogTitle>{t("assistantDock.convertTitle")}</DialogTitle>
          <DialogDescription>{t("assistantDock.convertDescription")}</DialogDescription>
        </DialogHeader>
        <form
          className="dialog-form"
          onSubmit={(event) => {
            event.preventDefault();
            void submit();
          }}
        >
          <Input
            aria-label={t("assistantDock.nameLabel")}
            autoFocus
            disabled={busy}
            onChange={(event) => setName(event.target.value)}
            placeholder={t("assistantDock.nameLabel")}
            value={name}
          />
          {error ? <p className="vk-field-error" role="alert">{error}</p> : null}
          <DialogFooter>
            <Button disabled={busy} onClick={() => props.onOpenChange(false)} type="button" variant="ghost">
              {t("conversation.action.cancel")}
            </Button>
            <Button disabled={busy || !name.trim()} type="submit">
              {busy ? <Spinner /> : null}
              {t("assistantDock.convert")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
