from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.app_teams_scope_body_apps_item import AppTeamsScopeBodyAppsItem
    from ..models.confirm import Confirm


T = TypeVar("T", bound="AppTeamsScopeBody")


@_attrs_define
class AppTeamsScopeBody:
    """
    Attributes:
        apps (Union[Unset, list['AppTeamsScopeBodyAppsItem']]):
        shared (Union[Unset, bool]):
        confirm (Union[Unset, Confirm]): A passkey assertion over the challenge a `412 CONFIRM_REQUIRED` named: the same
            request sent again with it approves exactly that request.
    """

    apps: Union[Unset, list["AppTeamsScopeBodyAppsItem"]] = UNSET
    shared: Union[Unset, bool] = UNSET
    confirm: Union[Unset, "Confirm"] = UNSET

    def to_dict(self) -> dict[str, Any]:
        from ..models.app_teams_scope_body_apps_item import AppTeamsScopeBodyAppsItem
        from ..models.confirm import Confirm

        apps: Union[Unset, list[dict[str, Any]]] = UNSET
        if not isinstance(self.apps, Unset):
            apps = []
            for apps_item_data in self.apps:
                apps_item = apps_item_data.to_dict()
                apps.append(apps_item)

        shared = self.shared

        confirm: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.confirm, Unset):
            confirm = self.confirm.to_dict()

        field_dict: dict[str, Any] = {}

        field_dict.update({})
        if apps is not UNSET:
            field_dict["apps"] = apps
        if shared is not UNSET:
            field_dict["shared"] = shared
        if confirm is not UNSET:
            field_dict["confirm"] = confirm

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.app_teams_scope_body_apps_item import AppTeamsScopeBodyAppsItem
        from ..models.confirm import Confirm

        d = dict(src_dict)
        apps = []
        _apps = d.pop("apps", UNSET)
        for apps_item_data in _apps or []:
            apps_item = AppTeamsScopeBodyAppsItem.from_dict(apps_item_data)

            apps.append(apps_item)

        shared = d.pop("shared", UNSET)

        _confirm = d.pop("confirm", UNSET)
        confirm: Union[Unset, Confirm]
        if isinstance(_confirm, Unset):
            confirm = UNSET
        else:
            confirm = Confirm.from_dict(_confirm)

        app_teams_scope_body = cls(
            apps=apps,
            shared=shared,
            confirm=confirm,
        )

        return app_teams_scope_body
