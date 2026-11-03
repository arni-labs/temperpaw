"""Build a deterministic archive; its SHA256 is the installed Temper runner setting."""

import argparse
import hashlib
import sys
import zipfile
from pathlib import Path


def build(destination: Path) -> str:
    source = Path(__file__).parent
    with zipfile.ZipFile(destination, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for name in ("runner.py", "product_flow.py", "__main__.py"):
            content = (
                b"from runner import main\nmain()\n"
                if name == "__main__.py"
                else (source / name).read_bytes()
            )
            info = zipfile.ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o500 << 16
            archive.writestr(info, content)
    return hashlib.sha256(destination.read_bytes()).hexdigest()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", type=Path)
    sys.stdout.write(build(parser.parse_args().destination) + "\n")
