FROM python:3.12-slim-bookworm
WORKDIR /tests
COPY tests/requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt && \
    useradd --uid 10002 --create-home tester && install -d -m 0700 -o tester -g tester /state
COPY tests/*.py ./
USER tester
CMD ["pytest", "-q", "-p", "no:cacheprovider"]
