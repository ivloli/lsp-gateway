const fs = require("fs");
const path = require("path");
const fetch = require("node-fetch");
const { HttpsProxyAgent } = require("https-proxy-agent");

const GATEWAY_URL = "http://0.0.0.0:8080/v1";
const requestDataPath = path.join(
  __dirname,
  "Captain-Eris_Violet-V0.420-12B-High-0124-short"
);

// Proxy configuration
// const PROXY_URL = "http://127.0.0.1:7890";
// const proxyAgent = new HttpsProxyAgent(PROXY_URL);

// console.log(`🔧 Using proxy: ${PROXY_URL}`);

/**
 * Parse SSE (Server-Sent Events) stream
 */
async function* parseSSEStream(response) {
  let buffer = "";
  let dataBuffer = ""; // Buffer for accumulating potentially split data lines

  try {
    // node-fetch uses Node.js streams, not Web Streams API
    for await (const chunk of response.body) {
      buffer += chunk.toString("utf8");

      // Process complete SSE messages
      const lines = buffer.split("\n");
      buffer = lines.pop() || ""; // Keep incomplete line in buffer

      for (const line of lines) {
        const trimmedLine = line.trim();

        if (trimmedLine.startsWith("data: ")) {
          const data = trimmedLine.slice(6); // Remove 'data: ' prefix

          if (data === "[DONE]") {
            console.log("\n📌 Received [DONE] marker");
            return;
          }

          if (data.trim()) {
            // Accumulate data in case JSON is split across multiple data: lines
            dataBuffer += data;

            // Try to parse the accumulated data
            try {
              const parsed = JSON.parse(dataBuffer);
              yield parsed;
              dataBuffer = ""; // Clear buffer on successful parse
            } catch (error) {
              // Check if this looks like incomplete JSON (ends mid-structure)
              const trimmedData = dataBuffer.trim();
              const endsIncomplete =
                !trimmedData.endsWith("}") &&
                !trimmedData.endsWith("]") &&
                trimmedData.length > 0;

              if (endsIncomplete && dataBuffer.length < 10000) {
                // Likely incomplete JSON, wait for more data
                // console.warn("⚠️  Incomplete JSON detected, waiting for more data...");
                continue;
              }

              // Looks complete but still fails to parse - this is a real error
              console.error("❌ JSON Parse Error:", error.message);
              console.error("   Raw data:", dataBuffer);
              //   console.error(
              //     "   Raw hex:",
              //     Buffer.from(dataBuffer)
              //       .toString("hex")
              //       .match(/.{1,2}/g)
              //       .join(" ")
              //   );
              const pos = parseInt(
                error.message.match(/position (\d+)/)?.[1] || "0"
              );
              console.error("   Position:", pos);
              if (pos > 0 && pos < dataBuffer.length) {
                console.error(
                  "   Char at position:",
                  dataBuffer[pos],
                  "(0x" + dataBuffer.charCodeAt(pos).toString(16) + ")"
                );
                console.error(
                  "   Context:",
                  dataBuffer.substring(
                    Math.max(0, pos - 20),
                    Math.min(dataBuffer.length, pos + 20)
                  )
                );
              }

              // Skip malformed chunks and continue processing
              console.warn("⚠️  Skipping malformed chunk and continuing...");
              dataBuffer = ""; // Clear the buffer
              continue;
            }
          }
        } else if (trimmedLine === "") {
          // Empty line might signal end of a message in SSE
          // If we have accumulated data, try to parse it now
          if (dataBuffer.trim()) {
            try {
              const parsed = JSON.parse(dataBuffer);
              yield parsed;
              dataBuffer = "";
            } catch (error) {
              // Still can't parse, skip it
              console.warn("⚠️  Failed to parse accumulated data, skipping...");
              dataBuffer = "";
            }
          }
        }
      }
    }

    console.log("\n✅ Stream completed");
  } catch (error) {
    console.error("\n❌ Stream error:", error.message);
    throw error;
  }
}

/**
 * Test streaming with Eris Violet model
 */
async function testErisVioletStreaming() {
  console.log("╔════════════════════════════════════════════════════════════╗");
  console.log("║  Testing Captain-Eris_Violet Model                        ║");
  console.log(
    "╚════════════════════════════════════════════════════════════╝\n"
  );

  const requestBody = JSON.parse(fs.readFileSync(requestDataPath, "utf8"));

  console.log("📋 Request Configuration:");
  console.log(`   Model: ${requestBody.model}`);
  console.log(`   Max Tokens: ${requestBody.max_tokens}`);
  console.log(`   Stop Sequences: ${JSON.stringify(requestBody.stop)}`);

  // Handle both messages array and prompt string formats
  if (requestBody.messages) {
    console.log(`   Messages: ${requestBody.messages.length} messages`);
  } else if (requestBody.prompt) {
    console.log(`   Prompt: ${requestBody.prompt.length} characters`);
  }
  console.log(`   Stream: ${requestBody.stream}`);

  // Show the last user message (if using messages format)
  if (requestBody.messages) {
    const lastUserMessage = requestBody.messages
      .filter((m) => m.role === "user")
      .pop();
    console.log(`\n💬 Last user message: "${lastUserMessage?.content}"\n`);
  } else if (requestBody.prompt) {
    // Show last 100 chars of prompt
    const promptPreview = requestBody.prompt.slice(-100);
    console.log(
      `\n💬 Prompt preview (last 100 chars): "...${promptPreview}"\n`
    );
  }

  const startTime = Date.now();
  let lastTokenTime = startTime;
  let response = "";
  let inputTokens = 0;
  let outputTokens = 0;
  let chunkCount = 0;
  let emptyChunkCount = 0;
  let stopReason = "";

  try {
    console.log("🚀 Starting streaming request...\n");

    const fetchResponse = await fetch(`${GATEWAY_URL}/chat/completions`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
      },
      body: JSON.stringify(requestBody),
      // agent: proxyAgent,
    });

    if (!fetchResponse.ok) {
      const errorText = await fetchResponse.text();
      throw new Error(
        `HTTP ${fetchResponse.status}: ${fetchResponse.statusText}\n${errorText}`
      );
    }

    console.log(`✅ Response status: ${fetchResponse.status}`);
    console.log("📡 Processing stream...\n");
    console.log("─".repeat(60));
    console.log("Response content:");
    console.log("─".repeat(60));

    // Setup timeout monitoring
    const tokenTimeoutMs = 10000; // 10 seconds
    const timeoutCheckInterval = tokenTimeoutMs / 3;

    const timeoutChecker = setInterval(() => {
      const timeSinceLastToken = Date.now() - lastTokenTime;
      if (timeSinceLastToken > tokenTimeoutMs) {
        console.error(
          `\n⏰ Token timeout: ${timeSinceLastToken}ms since last token (threshold: ${tokenTimeoutMs}ms)`
        );
      }
    }, timeoutCheckInterval);

    try {
      for await (const chunk of parseSSEStream(fetchResponse)) {
        chunkCount++;

        // Extract content
        const content = chunk.choices?.[0]?.delta?.content;

        if (content) {
          lastTokenTime = Date.now();
          response += content;
          process.stdout.write(content); // Stream output to console
        } else {
          emptyChunkCount++;
        }

        // Extract usage info
        if (chunk.usage) {
          inputTokens = chunk.usage.prompt_tokens || 0;
          outputTokens = chunk.usage.completion_tokens || 0;
        }

        // Extract finish reason
        if (chunk.choices?.[0]?.finish_reason) {
          stopReason = chunk.choices[0].finish_reason;
        }
      }
    } finally {
      clearInterval(timeoutChecker);
    }

    const totalTime = Date.now() - startTime;

    console.log("\n" + "─".repeat(60));
    console.log("\n📊 Stream Statistics:");
    console.log("─".repeat(60));
    console.log(`✅ Total chunks received: ${chunkCount}`);
    console.log(`📝 Empty chunks: ${emptyChunkCount}`);
    console.log(`📏 Response length: ${response.length} chars`);
    console.log(`🎯 Stop reason: ${stopReason || "none"}`);
    console.log(`⏱️  Total time: ${totalTime}ms`);
    console.log(`🔢 Input tokens: ${inputTokens}`);
    console.log(`🔢 Output tokens: ${outputTokens}`);
    console.log(`🔢 Total tokens: ${inputTokens + outputTokens}`);
    console.log("─".repeat(60));

    // Validation
    console.log("\n🔍 Validation:");
    console.log("─".repeat(60));

    const issues = [];

    if (!response) {
      issues.push("❌ Empty response");
    } else {
      console.log("✅ Response is not empty");
    }

    if (emptyChunkCount > 10) {
      issues.push(`⚠️  Too many empty chunks: ${emptyChunkCount}`);
    } else {
      console.log(
        `✅ Empty chunks within acceptable range: ${emptyChunkCount}`
      );
    }

    if (inputTokens === 0) {
      issues.push("⚠️  No input tokens reported");
    } else {
      console.log(`✅ Input tokens reported: ${inputTokens}`);
    }

    if (outputTokens === 0) {
      issues.push("⚠️  No output tokens reported");
    } else {
      console.log(`✅ Output tokens reported: ${outputTokens}`);
    }

    // Check if stop sequences were respected
    const stopSequences = requestBody.stop || [];
    const responseEndedWithStop = stopSequences.some((seq) =>
      response.includes(seq)
    );
    if (stopSequences.length > 0) {
      console.log(
        `${
          responseEndedWithStop ? "✅" : "ℹ️ "
        } Stop sequences: ${JSON.stringify(stopSequences)}`
      );
    }

    console.log("─".repeat(60));

    if (issues.length > 0) {
      console.log("\n⚠️  Issues found:");
      issues.forEach((issue) => console.log(`   ${issue}`));
      return 1;
    } else {
      console.log("\n🎉 All validations passed!");
      return 0;
    }
  } catch (error) {
    console.error("\n❌ Error:", error.message);
    if (error.stack) {
      console.error("\nStack trace:");
      console.error(error.stack);
    }
    return 1;
  }
}

// Run the test
testErisVioletStreaming()
  .then((exitCode) => {
    console.log(
      `\n${"═".repeat(60)}\nTest ${
        exitCode === 0 ? "PASSED ✅" : "FAILED ❌"
      }\n${"═".repeat(60)}`
    );
    process.exit(exitCode);
  })
  .catch((error) => {
    console.error("\n💥 Fatal error:", error);
    process.exit(1);
  });
