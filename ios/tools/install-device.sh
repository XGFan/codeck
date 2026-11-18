#!/usr/bin/env bash
# 把 iPhone App 装到真机：无签名 Release 构建，按证书组重签，经 devicectl 网络通道安装
# （手机与 Mac 已配对即可，免数据线）。
#
# 用法：
#   ios/tools/install-device.sh                         # 默认证书组、设备 "iPhone 18 Pro Max"
#   ios/tools/install-device.sh Release "我的手机"       # 指定设备名/UDID（只支持 Release）
#   ios/tools/install-device.sh --cert 15ProMax_KKZGSNKHD8 [Release] [设备]   # 改用别的证书组
#
# 证书组是一个目录：一份描述文件（*.mobileprovision）与 entitlements.plist，签名证书已导入钥匙串。
# --cert 给目录路径（含 /），或 IOS_CERT_ROOT 下的目录名；缺省用 IOS_CERT_ROOT 下的 CERT_GROUP。
# 换签名团队后 application-identifier 会变，iOS 不允许原地升级：先在手机上卸载旧版再装。
set -euo pipefail

CERT_GROUP=18ProMax_W4PQ9UWW7X
IOS_CERT_ROOT="${IOS_CERT_ROOT:-$HOME/Developer/Joy/ios-cert}"
ARGS=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --cert)
      [[ $# -ge 2 ]] || { echo "--cert 后面要跟证书组目录" >&2; exit 2; }
      CERT_GROUP="$2"; shift ;;
    --cert=*) CERT_GROUP="${1#--cert=}" ;;
    -*) echo "不认识的选项：$1" >&2; exit 2 ;;
    *) ARGS+=("$1") ;;
  esac
  shift
done

CONFIG="${ARGS[0]:-Release}"
DEVICE="${ARGS[1]:-iPhone 18 Pro Max}"
export DEVELOPER_DIR="${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}"

if [[ "$CONFIG" != Release ]]; then
  echo "重签只支持 Release：证书组的描述文件不允许调试，${CONFIG} 构建装不上。" >&2
  exit 2
fi

IOS_DIR="$(cd "$(dirname "$0")/.." && pwd)"

# 先找齐证书组的描述文件、entitlements 与签名身份，缺了就不必构建。
if [[ "$CERT_GROUP" == */* ]]; then CERT_DIR="$CERT_GROUP"; else CERT_DIR="$IOS_CERT_ROOT/$CERT_GROUP"; fi
[[ -d "$CERT_DIR" ]] || { echo "证书组目录不存在：$CERT_DIR" >&2; exit 1; }
shopt -s nullglob
profiles=("$CERT_DIR"/*.mobileprovision)
shopt -u nullglob
[[ ${#profiles[@]} -eq 1 ]] || { echo "证书组目录里要有且只有一份 .mobileprovision：$CERT_DIR" >&2; exit 1; }
PROFILE="${profiles[0]}"
ENTITLEMENTS="$CERT_DIR/entitlements.plist"
[[ -f "$ENTITLEMENTS" ]] || { echo "证书组目录里没有 entitlements.plist：$CERT_DIR" >&2; exit 1; }

# 签名身份：描述文件登记的证书里，钥匙串中有私钥的那一张，按 SHA-1 取。
profile_plist="$(security cms -D -i "$PROFILE")"
identities="$(security find-identity -v -p codesigning)"
SIGN=""
for ((i = 0; ; i++)); do
  der="$(plutil -extract "DeveloperCertificates.$i" raw -o - - <<<"$profile_plist" 2>/dev/null)" || break
  sha1="$(base64 -d <<<"$der" | shasum -a 1 | awk '{print toupper($1)}')"
  if grep -q "$sha1" <<<"$identities"; then SIGN="$sha1"; break; fi
done
[[ -n "$SIGN" ]] || { echo "钥匙串里没有描述文件对应的签名证书：$PROFILE" >&2; exit 1; }
SIGN_NAME="$(grep "$SIGN" <<<"$identities" | head -1 | sed -E 's/.*"(.*)"/\1/')"

cd "$IOS_DIR"
DERIVED="$IOS_DIR/build/device"
APP="$DERIVED/Build/Products/Release-iphoneos/CoDeck.app"
xcodebuild -project CoDeck.xcodeproj -scheme CoDeck -configuration Release \
  -destination 'generic/platform=iOS' -derivedDataPath "$DERIVED" \
  CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO CODE_SIGN_IDENTITY="" build

# 塞描述文件后由内向外签：内嵌的 framework 先签（现在没有），最后签 .app 本体并带上 entitlements。
cp "$PROFILE" "$APP/embedded.mobileprovision"
shopt -s nullglob
for fw in "$APP"/Frameworks/*.framework "$APP"/Frameworks/*.dylib; do
  codesign --force --sign "$SIGN" --timestamp=none "$fw"
done
shopt -u nullglob
codesign --force --sign "$SIGN" --entitlements "$ENTITLEMENTS" --timestamp=none "$APP"
codesign --verify --deep --strict "$APP"
echo "已用「${SIGN_NAME}」重签（证书组 ${CERT_DIR}）"

if ! xcrun devicectl device install app --device "$DEVICE" "$APP"; then
  echo "安装失败。手机上的 CoDeck 若是别的团队签的，先在手机上卸载再重跑。" >&2
  exit 1
fi
echo "已安装重签的 Release 构建到「${DEVICE}」：${APP}"
