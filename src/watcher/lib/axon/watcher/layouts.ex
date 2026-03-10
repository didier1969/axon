defmodule Axon.Watcher.Layouts do
  use Phoenix.Component

  def root(assigns) do
    ~H"""
    <!DOCTYPE html>
    <html lang="en">
      <head>
        <meta charset="utf-8"/>
        <meta name="viewport" content="width=device-width, initial-scale=1"/>
        <meta name="csrf-token" content="axon_no_csrf" />
        <title>Axon Cockpit v1.0</title>
        <style>
          body { background: #0a0a0b; color: #e4e4e7; font-family: 'Segoe UI', system-ui, sans-serif; margin: 0; padding: 20px; }
          .container { max-width: 1200px; margin: 0 auto; }
          .header { display: flex; justify-content: space-between; align-items: center; border-bottom: 1px solid #27272a; padding-bottom: 20px; margin-bottom: 30px; }
          .status-badge { padding: 4px 12px; border-radius: 9999px; font-size: 0.75rem; font-weight: 600; text-transform: uppercase; }
          .status-live { background: #064e3b; color: #34d399; }
          .status-error { background: #7f1d1d; color: #f87171; }
          .grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(300px, 1fr)); gap: 20px; }
          .card { background: #18181b; border: 1px solid #27272a; border-radius: 8px; padding: 20px; }
          .card-title { font-size: 1.125rem; font-weight: 600; margin-bottom: 15px; display: flex; align-items: center; gap: 8px; }
          .stat { margin-bottom: 10px; font-size: 0.875rem; color: #a1a1aa; }
          .stat span { color: #f4f4f5; font-weight: 500; }
          .progress-bar { height: 6px; background: #27272a; border-radius: 3px; overflow: hidden; margin-top: 15px; }
          .progress-fill { height: 100%; background: #3b82f6; transition: width 0.3s ease; }
          .pulse { width: 10px; height: 10px; border-radius: 50%; background: #34d399; display: inline-block; animation: pulse 2s infinite; }
          @keyframes pulse { 0% { opacity: 1; } 50% { opacity: 0.3; } 100% { opacity: 1; } }
        </style>
      </head>
      <body>
        <div class="container">
          {@inner_content}
        </div>
        <script src="/live/phoenix_live_view.js"></script>
        <script>
          /* Minimal JS for LiveView connection if needed */
        </script>
      </body>
    </html>
    """
  end
end
