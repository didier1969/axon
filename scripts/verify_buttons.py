import asyncio
from playwright.async_api import async_playwright

async def verify_cockpit():
    async with async_playwright() as p:
        browser = await p.chromium.launch(headless=True)
        page = await browser.new_page()

        print("🌐 Navigating to Cockpit...")
        await page.goto("http://localhost:6061/cockpit")
        await page.wait_for_load_state("networkidle")
        await asyncio.sleep(2)

        print("🔘 Clicking 'Execute Full Scan'...")
        button = page.locator("button", has_text="Execute Full Scan")
        if await button.count() > 0:
            await button.click()
            print("✅ Clicked! Waiting 5 seconds for processing to populate the directory map...")
            await asyncio.sleep(5)
            
            print("📸 Taking screenshot of the Live Directory Map...")
            await page.screenshot(path="logs/cockpit_directory_map.png", full_page=True)
            print("✅ Screenshot saved to logs/cockpit_directory_map.png")
            
            content = await page.content()
            if "DONE" in content or "TOTAL" in content:
                print("✅ SUCCESS: The Directory Map is populated with real-time data!")
            else:
                print("⚠️ Warning: Directory Map might still be pending.")
        else:
            print("❌ Button not found!")

        await browser.close()

if __name__ == "__main__":
    asyncio.run(verify_cockpit())