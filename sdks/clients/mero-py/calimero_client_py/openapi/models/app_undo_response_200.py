from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.undo_part import UndoPart
    from ..models.app_undo_response_200_groups import AppUndoResponse200Groups


T = TypeVar("T", bound="AppUndoResponse200")


@_attrs_define
class AppUndoResponse200:
    """
    Attributes:
        app (str):
        groups (AppUndoResponse200Groups):
        restored (bool):
        apps (list['UndoPart']):
    """

    app: str
    groups: "AppUndoResponse200Groups"
    restored: bool
    apps: list["UndoPart"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.undo_part import UndoPart
        from ..models.app_undo_response_200_groups import AppUndoResponse200Groups

        app = self.app

        groups = self.groups.to_dict()

        restored = self.restored

        apps = []
        for apps_item_data in self.apps:
            apps_item = apps_item_data.to_dict()
            apps.append(apps_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "app": app,
                "groups": groups,
                "restored": restored,
                "apps": apps,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.undo_part import UndoPart
        from ..models.app_undo_response_200_groups import AppUndoResponse200Groups

        d = dict(src_dict)
        app = d.pop("app")

        groups = AppUndoResponse200Groups.from_dict(d.pop("groups"))

        restored = d.pop("restored")

        apps = []
        _apps = d.pop("apps")
        for apps_item_data in _apps:
            apps_item = UndoPart.from_dict(apps_item_data)

            apps.append(apps_item)

        app_undo_response_200 = cls(
            app=app,
            groups=groups,
            restored=restored,
            apps=apps,
        )

        app_undo_response_200.additional_properties = d
        return app_undo_response_200

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
