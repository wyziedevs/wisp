<title>{status}</title>

<div class="text-column error">
  <img class="vanish" src="/favicon.svg" alt="" title="Casper" width="72" height="72">
  <h1>{status}</h1>
  {#if status == 404}
    <p>This page has vanished into thin air.</p>
  {:else}
    <p>{message}</p>
  {/if}
  <p><a class="button" href="/" title="Back to the Home Page">Back to the Home Page</a></p>
</div>
