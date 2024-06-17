async function toggle_led2() {
    let response = await fetch("/toggle_led/2");
    let response_text = await response.text();
    let cleanText = response_text.replace(/"/g, '');
    document.getElementById("led2Label").innerText = cleanText;
}
