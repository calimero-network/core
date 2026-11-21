from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.app_update_response_200_approvers import AppUpdateResponse200Approvers
from typing import cast
from typing import cast, Union

if TYPE_CHECKING:
    from ..models.pending_approval import PendingApproval
    from ..models.app_update_response_200_running_type_0 import (
        AppUpdateResponse200RunningType0,
    )
    from ..models.app_update_response_200_available_type_0 import (
        AppUpdateResponse200AvailableType0,
    )
    from ..models.app_update_response_200_policy import AppUpdateResponse200Policy


T = TypeVar("T", bound="AppUpdateResponse200")


@_attrs_define
class AppUpdateResponse200:
    """
    Attributes:
        app (str):
        running (Union['AppUpdateResponse200RunningType0', None]):
        available (Union['AppUpdateResponse200AvailableType0', None]):
        unit (bool):
        may_approve (bool):
        approvers (AppUpdateResponse200Approvers):
        policy (AppUpdateResponse200Policy):
        pending_approvals (list['PendingApproval']):
    """

    app: str
    running: Union["AppUpdateResponse200RunningType0", None]
    available: Union["AppUpdateResponse200AvailableType0", None]
    unit: bool
    may_approve: bool
    approvers: AppUpdateResponse200Approvers
    policy: "AppUpdateResponse200Policy"
    pending_approvals: list["PendingApproval"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.pending_approval import PendingApproval
        from ..models.app_update_response_200_running_type_0 import (
            AppUpdateResponse200RunningType0,
        )
        from ..models.app_update_response_200_available_type_0 import (
            AppUpdateResponse200AvailableType0,
        )
        from ..models.app_update_response_200_policy import AppUpdateResponse200Policy

        app = self.app

        running: Union[None, dict[str, Any]]
        if isinstance(self.running, AppUpdateResponse200RunningType0):
            running = self.running.to_dict()
        else:
            running = self.running

        available: Union[None, dict[str, Any]]
        if isinstance(self.available, AppUpdateResponse200AvailableType0):
            available = self.available.to_dict()
        else:
            available = self.available

        unit = self.unit

        may_approve = self.may_approve

        approvers = self.approvers.value

        policy = self.policy.to_dict()

        pending_approvals = []
        for pending_approvals_item_data in self.pending_approvals:
            pending_approvals_item = pending_approvals_item_data.to_dict()
            pending_approvals.append(pending_approvals_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "app": app,
                "running": running,
                "available": available,
                "unit": unit,
                "mayApprove": may_approve,
                "approvers": approvers,
                "policy": policy,
                "pendingApprovals": pending_approvals,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.pending_approval import PendingApproval
        from ..models.app_update_response_200_running_type_0 import (
            AppUpdateResponse200RunningType0,
        )
        from ..models.app_update_response_200_available_type_0 import (
            AppUpdateResponse200AvailableType0,
        )
        from ..models.app_update_response_200_policy import AppUpdateResponse200Policy

        d = dict(src_dict)
        app = d.pop("app")

        def _parse_running(
            data: object,
        ) -> Union["AppUpdateResponse200RunningType0", None]:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                running_type_0 = AppUpdateResponse200RunningType0.from_dict(data)

                return running_type_0
            except:  # noqa: E722
                pass
            return cast(Union["AppUpdateResponse200RunningType0", None], data)

        running = _parse_running(d.pop("running"))

        def _parse_available(
            data: object,
        ) -> Union["AppUpdateResponse200AvailableType0", None]:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                available_type_0 = AppUpdateResponse200AvailableType0.from_dict(data)

                return available_type_0
            except:  # noqa: E722
                pass
            return cast(Union["AppUpdateResponse200AvailableType0", None], data)

        available = _parse_available(d.pop("available"))

        unit = d.pop("unit")

        may_approve = d.pop("mayApprove")

        approvers = AppUpdateResponse200Approvers(d.pop("approvers"))

        policy = AppUpdateResponse200Policy.from_dict(d.pop("policy"))

        pending_approvals = []
        _pending_approvals = d.pop("pendingApprovals")
        for pending_approvals_item_data in _pending_approvals:
            pending_approvals_item = PendingApproval.from_dict(
                pending_approvals_item_data
            )

            pending_approvals.append(pending_approvals_item)

        app_update_response_200 = cls(
            app=app,
            running=running,
            available=available,
            unit=unit,
            may_approve=may_approve,
            approvers=approvers,
            policy=policy,
            pending_approvals=pending_approvals,
        )

        app_update_response_200.additional_properties = d
        return app_update_response_200

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
