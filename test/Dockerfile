# Mock Server Dockerfile
FROM python:3.11-slim

# Set working directory
WORKDIR /app

# Install system dependencies
RUN apt-get update && apt-get install -y \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Copy requirements first for better layer caching
COPY requirements.txt .

# Install Python dependencies
RUN pip install --no-cache-dir -r requirements.txt

# Copy the mock server code
COPY mock_server.py .

# Create non-root user for security
RUN useradd -r -s /bin/false -m -d /app mockuser && \
    chown -R mockuser:mockuser /app

# Switch to non-root user
USER mockuser

# Expose port
EXPOSE 8000

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8000/normal || exit 1

# Set default environment variables
ENV PYTHONUNBUFFERED=1
ENV PYTHONDONTWRITEBYTECODE=1

# Default command with configurable probabilities
CMD ["python", "mock_server.py", "--normal", "0.5", "--error", "0.3", "--timeout", "0.2"]
