<script lang="ts" module>
  // Confirmation with shadcn's AlertDialog: `await confirm({...})` returns true/false.
  interface Request {
    title: string;
    description: string;
    action: string;
    resolve: (ok: boolean) => void;
  }

  let request = $state<Request | null>(null);

  export function confirm(opts: Omit<Request, "resolve">): Promise<boolean> {
    request?.resolve(false);
    return new Promise((resolve) => (request = { ...opts, resolve }));
  }
</script>

<script lang="ts">
  import * as AlertDialog from "$lib/components/ui/alert-dialog";

  function close(ok: boolean) {
    request?.resolve(ok);
    request = null;
  }
</script>

<AlertDialog.Root open={request !== null} onOpenChange={(open) => !open && close(false)}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>{request?.title}</AlertDialog.Title>
      <AlertDialog.Description>{request?.description}</AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel onclick={() => close(false)}>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action variant="destructive" onclick={() => close(true)}>{request?.action}</AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
