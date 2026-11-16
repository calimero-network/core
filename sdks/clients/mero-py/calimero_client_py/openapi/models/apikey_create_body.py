from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.apikey_create_body_upgrades import ApikeyCreateBodyUpgrades
    from ..models.confirm import Confirm


T = TypeVar("T", bound="ApikeyCreateBody")


@_attrs_define
class ApikeyCreateBody:
    """
    Attributes:
        label (Union[Unset, str]):
        apps (Union[Unset, list[str]]):
        methods (Union[Unset, list[str]]):
        read_only (Union[Unset, bool]):
        allow_confirm (Union[Unset, bool]):
        ttl_secs (Union[Unset, int]):
        upgrades (Union[Unset, ApikeyCreateBodyUpgrades]):
        confirm (Union[Unset, Confirm]): A passkey assertion over the challenge a `412 CONFIRM_REQUIRED` named: the same
            request sent again with it approves exactly that request.
    """

    label: Union[Unset, str] = UNSET
    apps: Union[Unset, list[str]] = UNSET
    methods: Union[Unset, list[str]] = UNSET
    read_only: Union[Unset, bool] = UNSET
    allow_confirm: Union[Unset, bool] = UNSET
    ttl_secs: Union[Unset, int] = UNSET
    upgrades: Union[Unset, "ApikeyCreateBodyUpgrades"] = UNSET
    confirm: Union[Unset, "Confirm"] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.apikey_create_body_upgrades import ApikeyCreateBodyUpgrades
        from ..models.confirm import Confirm

        label = self.label

        apps: Union[Unset, list[str]] = UNSET
        if not isinstance(self.apps, Unset):
            apps = self.apps

        methods: Union[Unset, list[str]] = UNSET
        if not isinstance(self.methods, Unset):
            methods = self.methods

        read_only = self.read_only

        allow_confirm = self.allow_confirm

        ttl_secs = self.ttl_secs

        upgrades: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.upgrades, Unset):
            upgrades = self.upgrades.to_dict()

        confirm: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.confirm, Unset):
            confirm = self.confirm.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if label is not UNSET:
            field_dict["label"] = label
        if apps is not UNSET:
            field_dict["apps"] = apps
        if methods is not UNSET:
            field_dict["methods"] = methods
        if read_only is not UNSET:
            field_dict["readOnly"] = read_only
        if allow_confirm is not UNSET:
            field_dict["allowConfirm"] = allow_confirm
        if ttl_secs is not UNSET:
            field_dict["ttlSecs"] = ttl_secs
        if upgrades is not UNSET:
            field_dict["upgrades"] = upgrades
        if confirm is not UNSET:
            field_dict["confirm"] = confirm

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.apikey_create_body_upgrades import ApikeyCreateBodyUpgrades
        from ..models.confirm import Confirm

        d = dict(src_dict)
        label = d.pop("label", UNSET)

        apps = cast(list[str], d.pop("apps", UNSET))

        methods = cast(list[str], d.pop("methods", UNSET))

        read_only = d.pop("readOnly", UNSET)

        allow_confirm = d.pop("allowConfirm", UNSET)

        ttl_secs = d.pop("ttlSecs", UNSET)

        _upgrades = d.pop("upgrades", UNSET)
        upgrades: Union[Unset, ApikeyCreateBodyUpgrades]
        if isinstance(_upgrades, Unset):
            upgrades = UNSET
        else:
            upgrades = ApikeyCreateBodyUpgrades.from_dict(_upgrades)

        _confirm = d.pop("confirm", UNSET)
        confirm: Union[Unset, Confirm]
        if isinstance(_confirm, Unset):
            confirm = UNSET
        else:
            confirm = Confirm.from_dict(_confirm)

        apikey_create_body = cls(
            label=label,
            apps=apps,
            methods=methods,
            read_only=read_only,
            allow_confirm=allow_confirm,
            ttl_secs=ttl_secs,
            upgrades=upgrades,
            confirm=confirm,
        )

        apikey_create_body.additional_properties = d
        return apikey_create_body

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
