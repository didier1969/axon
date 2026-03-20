import asyncio
from playwright.async_api import async_playwright
import time

async def diagnose():
    async with async_playwright() as p:
        browser = await p.chromium.launch(headless=True)
        context = await browser.new_context()
        page = await context.new_page()

        console_logs = []
        page.on("console", lambda msg: console_logs.append(f"[{msg.type}] {msg.text}"))
        page.on("pageerror", lambda exc: console_logs.append(f"[EXCEPTION] {exc}"))

        print("🌐 Navigating to Cockpit...")
        try:
            await page.goto("http://localhost:44127/cockpit", timeout=10000)
            await page.wait_for_load_state("networkidle")
            
            # Attendre que LiveView soit monté
            await asyncio.sleep(2)
            
            print("📸 Taking initial screenshot...")
            await page.screenshot(path="logs/cockpit_initial.png")

            print("🔘 Clicking 'Execute Full Scan' button...")
            # On cherche le bouton par son texte (ignorer la casse car c'est en CSS uppercase maintenant)
            button = page.get_by_text("Execute Full Scan")
            if await button.count() > 0:
                await button.click()
                print("✅ Clicked!")
            else:
                print("❌ Button not found!")

            # Attendre une réaction
            await asyncio.sleep(3)
            
            print("📸 Taking final screenshot...")
            await page.screenshot(path="logs/cockpit_final.png")

            print("\n📜 CONSOLE LOGS CAPTURED:")
            for log in console_logs:
                print(log)

        except Exception as e:
            print(f"❌ Error during diagnosis: {e}")
        finally:
            await browser.close()

if __name__ == "__main__":
    asyncio.run(diagnose())
