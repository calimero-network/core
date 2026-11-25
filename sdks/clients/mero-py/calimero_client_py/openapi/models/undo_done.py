from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.undo_part import UndoPart
    from ..models.undo_done_groups import UndoDoneGroups


T = TypeVar("T", bound="UndoDone")


@_attrs_define
class UndoDone:
    """What `POST /apps/{app}/undo` answers: each app group restored (its restore point's index, `null` when none), whether
    every group takes calls again, and each app reverted.

        Attributes:
            app (str):
            groups (UndoDoneGroups):
            restored (bool):
            apps (list['UndoPart']):
    """

    app: str
    groups: "UndoDoneGroups"
    restored: bool
    apps: list["UndoPart"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.undo_part import UndoPart
        from ..models.undo_done_groups import UndoDoneGroups

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
        from ..models.undo_done_groups import UndoDoneGroups

        d = dict(src_dict)
        app = d.pop("app")

        groups = UndoDoneGroups.from_dict(d.pop("groups"))

        restored = d.pop("restored")

        apps = []
        _apps = d.pop("apps")
        for apps_item_data in _apps:
            apps_item = UndoPart.from_dict(apps_item_data)

            apps.append(apps_item)

        undo_done = cls(
            app=app,
            groups=groups,
            restored=restored,
            apps=apps,
        )

        undo_done.additional_properties = d
        return undo_done

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
