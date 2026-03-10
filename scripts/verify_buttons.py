import asyncio
from playwright.async_api import async_playwright

async def verify_cockpit():
    async with async_playwright() as p:
        browser = await p.chromium.launch(headless=True)
        page = await browser.new_page()

        print("🌐 Navigating to Cockpit...")
        await page.goto("http://localhost:6061/cockpit")
        await asyncio.sleep(2)

        print("📊 Checking UI selectors...")
        try:
            # We use a more generic selector for progress
            progress_locator = page.locator("span:has-text('%')")
            initial_progress = await progress_locator.first.inner_text()
            print(f"   Initial Progress: {initial_progress}")

            print("🔘 Clicking 'Execute Full Scan'...")
            # We look for the button by its exact text
            button = page.locator("button", has_text="Execute Full Scan")
            await button.click()
            
            print("⏳ Waiting for UI update (10s)...")
            # We wait for the progress to change or for "FILES" to appear in the map
            await asyncio.sleep(10)

            final_progress = await progress_locator.first.inner_text()
            print(f"   Final Progress: {final_progress}")
            
            content = await page.content()
            if "FILES" in content or final_progress != initial_progress:
                print("✅ SUCCESS: The system reacted to the click!")
            else:
                print("❌ FAILURE: No reaction detected in the UI.")
                # If failure, let's see if the button is even clickable
                is_disabled = await button.is_disabled()
                print(f"   Button disabled? {is_disabled}")

        except Exception as e:
            print(f"❌ Error during test: {e}")
        
        await browser.close()

if __name__ == "__main__":
    asyncio.run(verify_cockpit())
