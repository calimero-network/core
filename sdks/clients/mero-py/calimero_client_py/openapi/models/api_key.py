from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import cast, Union
from typing import Union

if TYPE_CHECKING:
    from ..models.api_key_upgrades import ApiKeyUpgrades


T = TypeVar("T", bound="ApiKey")


@_attrs_define
class ApiKey:
    """An API key as the account's list shows it (never its secret): `apps` and `methods` are `null` for every one;
    `upgrades` makes it an upgrade key.

        Attributes:
            key_id (str):
            account (str):
            label (str):
            apps (Union[None, list[str]]):
            methods (Union[None, list[str]]):
            read_only (bool):
            allow_confirm (bool):
            expires_at (int):
            upgrades (Union[Unset, ApiKeyUpgrades]):
    """

    key_id: str
    account: str
    label: str
    apps: Union[None, list[str]]
    methods: Union[None, list[str]]
    read_only: bool
    allow_confirm: bool
    expires_at: int
    upgrades: Union[Unset, "ApiKeyUpgrades"] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.api_key_upgrades import ApiKeyUpgrades

        key_id = self.key_id

        account = self.account

        label = self.label

        apps: Union[None, list[str]]
        if isinstance(self.apps, list):
            apps = self.apps

        else:
            apps = self.apps

        methods: Union[None, list[str]]
        if isinstance(self.methods, list):
            methods = self.methods

        else:
            methods = self.methods

        read_only = self.read_only

        allow_confirm = self.allow_confirm

        expires_at = self.expires_at

        upgrades: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.upgrades, Unset):
            upgrades = self.upgrades.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "keyId": key_id,
                "account": account,
                "label": label,
                "apps": apps,
                "methods": methods,
                "readOnly": read_only,
                "allowConfirm": allow_confirm,
                "expiresAt": expires_at,
            }
        )
        if upgrades is not UNSET:
            field_dict["upgrades"] = upgrades

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.api_key_upgrades import ApiKeyUpgrades

        d = dict(src_dict)
        key_id = d.pop("keyId")

        account = d.pop("account")

        label = d.pop("label")

        def _parse_apps(data: object) -> Union[None, list[str]]:
            if data is None:
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                apps_type_0 = cast(list[str], data)

                return apps_type_0
            except:  # noqa: E722
                pass
            return cast(Union[None, list[str]], data)

        apps = _parse_apps(d.pop("apps"))

        def _parse_methods(data: object) -> Union[None, list[str]]:
            if data is None:
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                methods_type_0 = cast(list[str], data)

                return methods_type_0
            except:  # noqa: E722
                pass
            return cast(Union[None, list[str]], data)

        methods = _parse_methods(d.pop("methods"))

        read_only = d.pop("readOnly")

        allow_confirm = d.pop("allowConfirm")

        expires_at = d.pop("expiresAt")

        _upgrades = d.pop("upgrades", UNSET)
        upgrades: Union[Unset, ApiKeyUpgrades]
        if isinstance(_upgrades, Unset):
            upgrades = UNSET
        else:
            upgrades = ApiKeyUpgrades.from_dict(_upgrades)

        api_key = cls(
            key_id=key_id,
            account=account,
            label=label,
            apps=apps,
            methods=methods,
            read_only=read_only,
            allow_confirm=allow_confirm,
            expires_at=expires_at,
            upgrades=upgrades,
        )

        api_key.additional_properties = d
        return api_key

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> Any:
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
