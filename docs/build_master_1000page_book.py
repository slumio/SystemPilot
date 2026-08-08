#!/usr/bin/env python3
import os
import subprocess
import sys

def compile_master_book():
    book_dir = "/home/joyboy/Systempilot/syspilot/docs/book"
    docs_dir = "/home/joyboy/Systempilot/syspilot/docs"
    artifacts_dir = "/home/joyboy/.gemini/antigravity/brain/e7a167b0-7e6c-4cd3-82af-7b3a6c37c406"

    os.makedirs(docs_dir, exist_ok=True)
    os.makedirs(artifacts_dir, exist_ok=True)

    v1_path = os.path.join(book_dir, "VOLUME_1_KERNEL_AND_EBPF_SUBSYSTEMS.md")
    v2_path = os.path.join(book_dir, "VOLUME_2_DAEMON_PIPELINE_AND_CLASS_SPECIFICATIONS.md")
    v3_path = os.path.join(book_dir, "VOLUME_3_COLUMNAR_TSDB_STORAGE_AND_INDEXING.md")
    v4_path = os.path.join(book_dir, "VOLUME_4_CAUSAL_GRAPH_ENGINE_IPC_AND_SAFETY.md")

    v1_content = open(v1_path, "r", encoding="utf-8").read()
    v2_content = open(v2_path, "r", encoding="utf-8").read()
    v3_content = open(v3_path, "r", encoding="utf-8").read()
    v4_content = open(v4_path, "r", encoding="utf-8").read()

    master_md_path = os.path.join(docs_dir, "SYSPILOT_MASTER_ARCHITECTURE_BOOK.md")
    with open(master_md_path, "w", encoding="utf-8") as f:
        f.write("# SysPilot Master Architecture & Systems Specification Manual\n\n")
        f.write(v1_content + "\n\n<div class=\"page-break\"></div>\n\n")
        f.write(v2_content + "\n\n<div class=\"page-break\"></div>\n\n")
        f.write(v3_content + "\n\n<div class=\"page-break\"></div>\n\n")
        f.write(v4_content + "\n")

    print(f"[+] Master Markdown compiled to: {master_md_path}")

    # Build HTML document
    css_styles = """
    <style>
        @page {
            size: A4;
            margin: 20mm 15mm 20mm 15mm;
            @bottom-right {
                content: "Page " counter(page);
                font-family: 'Segoe UI', Arial, sans-serif;
                font-size: 8.5pt;
                color: #64748b;
            }
        }
        body {
            font-family: 'Segoe UI', -apple-system, BlinkMacSystemFont, Roboto, sans-serif;
            font-size: 10pt;
            line-height: 1.55;
            color: #0f172a;
            background-color: #ffffff;
        }
        .cover-page {
            page-break-after: always;
            text-align: center;
            padding-top: 100px;
        }
        .cover-title {
            font-size: 34pt;
            font-weight: 800;
            color: #0f172a;
            letter-spacing: -0.5px;
            margin-bottom: 20px;
        }
        .cover-subtitle {
            font-size: 16pt;
            color: #0284c7;
            margin-bottom: 50px;
        }
        h1 {
            font-size: 20pt;
            font-weight: 700;
            color: #0f172a;
            border-bottom: 2px solid #0284c7;
            padding-bottom: 6px;
            margin-top: 35px;
            page-break-before: always;
        }
        h2 {
            font-size: 14pt;
            font-weight: 600;
            color: #0369a1;
            margin-top: 22px;
            border-bottom: 1px solid #e2e8f0;
            padding-bottom: 4px;
        }
        h3 {
            font-size: 11pt;
            font-weight: 600;
            color: #334155;
            margin-top: 16px;
        }
        pre {
            background-color: #0f172a;
            color: #f8fafc;
            padding: 12px;
            border-radius: 6px;
            font-family: 'JetBrains Mono', 'Fira Code', monospace;
            font-size: 8pt;
            line-height: 1.4;
            page-break-inside: avoid;
            margin: 14px 0;
        }
        code {
            font-family: 'JetBrains Mono', 'Fira Code', monospace;
            font-size: 9pt;
            background-color: #f1f5f9;
            color: #0f172a;
            padding: 2px 4px;
            border-radius: 4px;
        }
        table {
            width: 100%;
            border-collapse: collapse;
            margin: 16px 0;
            font-size: 9pt;
            page-break-inside: avoid;
        }
        th, td {
            border: 1px solid #cbd5e1;
            padding: 7px 10px;
            text-align: left;
        }
        th {
            background-color: #f8fafc;
            font-weight: 700;
            color: #0f172a;
        }
        tr:nth-child(even) {
            background-color: #f8fafc;
        }
        .page-break {
            page-break-after: always;
        }
    </style>
    """

    # Format Markdown content into styled HTML
    html_file = os.path.join(docs_dir, "syspilot_master_1000page_book.html")
    pdf_file_docs = os.path.join(docs_dir, "syspilot_master_1000page_book.pdf")
    pdf_file_artifact = os.path.join(artifacts_dir, "syspilot_master_1000page_book.pdf")

    # Simple Markdown to HTML converter for book structure
    import re
    body_html = open(master_md_path, "r", encoding="utf-8").read()
    
    # Process headers
    body_html = re.sub(r'^# (.*?)$', r'<h1>\1</h1>', body_html, flags=re.MULTILINE)
    body_html = re.sub(r'^## (.*?)$', r'<h2>\1</h2>', body_html, flags=re.MULTILINE)
    body_html = re.sub(r'^### (.*?)$', r'<h3>\1</h3>', body_html, flags=re.MULTILINE)

    html_content = f"""<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <title>SysPilot Master Systems Architecture Manual</title>
    {css_styles}
</head>
<body>
<div class="cover-page">
    <div class="cover-title">SysPilot Master Architecture Specification</div>
    <div class="cover-subtitle">Complete 4-Volume Technical Reference Manual for Production Linux Observability</div>
    <p><strong>Kernel eBPF Probes · Edge Daemon Pipeline · Columnar TSDB · Causal Graph Engine</strong></p>
    <div style="margin-top: 60px; font-size: 10pt; color: #475569;">
        <p><strong>Document Ref:</strong> SIOP-MASTER-BOOK-2026</p>
        <p><strong>Status:</strong> Complete Systems Design & Class Catalog</p>
    </div>
</div>
{body_html}
</body>
</html>
"""

    with open(html_file, "w", encoding="utf-8") as f:
        f.write(html_content)

    print(f"[+] Master HTML book written to: {html_file}")

    print("[*] Converting master book to PDF via LibreOffice...")
    cmd = [
        "libreoffice", "--headless", "--convert-to", "pdf",
        html_file, "--outdir", docs_dir
    ]
    res = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    if res.returncode == 0:
        print(f"[+] Successfully generated PDF: {pdf_file_docs}")
        subprocess.run(["cp", pdf_file_docs, pdf_file_artifact], check=True)
        print(f"[+] Successfully copied PDF artifact to: {pdf_file_artifact}")
    else:
        print(f"[-] PDF conversion error: {res.stderr}")
        sys.exit(1)

if __name__ == "__main__":
    compile_master_book()
