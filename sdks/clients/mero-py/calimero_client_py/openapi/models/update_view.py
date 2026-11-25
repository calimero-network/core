from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.update_view_approvers import UpdateViewApprovers
from typing import cast
from typing import cast, Union

if TYPE_CHECKING:
    from ..models.update_view_running_type_0 import UpdateViewRunningType0
    from ..models.deployment_shown import DeploymentShown
    from ..models.update_view_available_type_0 import UpdateViewAvailableType0
    from ..models.update_view_policy import UpdateViewPolicy
    from ..models.pending_approval import PendingApproval


T = TypeVar("T", bound="UpdateView")


@_attrs_define
class UpdateView:
    """`GET /apps/{app}/update`: the release the instance runs, the update available to it (with the classifier's verdict),
    who may approve it, the policy in force and the live first approvals.

        Attributes:
            app (str):
            running (Union['UpdateViewRunningType0', None]):
            available (Union['UpdateViewAvailableType0', None]):
            unit (bool):
            may_approve (bool):
            approvers (UpdateViewApprovers):
            policy (UpdateViewPolicy):
            pending_approvals (list['PendingApproval']):
            deployment (Union['DeploymentShown', None]):
    """

    app: str
    running: Union["UpdateViewRunningType0", None]
    available: Union["UpdateViewAvailableType0", None]
    unit: bool
    may_approve: bool
    approvers: UpdateViewApprovers
    policy: "UpdateViewPolicy"
    pending_approvals: list["PendingApproval"]
    deployment: Union["DeploymentShown", None]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.update_view_running_type_0 import UpdateViewRunningType0
        from ..models.deployment_shown import DeploymentShown
        from ..models.update_view_available_type_0 import UpdateViewAvailableType0
        from ..models.update_view_policy import UpdateViewPolicy
        from ..models.pending_approval import PendingApproval

        app = self.app

        running: Union[None, dict[str, Any]]
        if isinstance(self.running, UpdateViewRunningType0):
            running = self.running.to_dict()
        else:
            running = self.running

        available: Union[None, dict[str, Any]]
        if isinstance(self.available, UpdateViewAvailableType0):
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

        deployment: Union[None, dict[str, Any]]
        if isinstance(self.deployment, DeploymentShown):
            deployment = self.deployment.to_dict()
        else:
            deployment = self.deployment

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
                "deployment": deployment,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.update_view_running_type_0 import UpdateViewRunningType0
        from ..models.deployment_shown import DeploymentShown
        from ..models.update_view_available_type_0 import UpdateViewAvailableType0
        from ..models.update_view_policy import UpdateViewPolicy
        from ..models.pending_approval import PendingApproval

        d = dict(src_dict)
        app = d.pop("app")

        def _parse_running(data: object) -> Union["UpdateViewRunningType0", None]:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                running_type_0 = UpdateViewRunningType0.from_dict(data)

                return running_type_0
            except:  # noqa: E722
                pass
            return cast(Union["UpdateViewRunningType0", None], data)

        running = _parse_running(d.pop("running"))

        def _parse_available(data: object) -> Union["UpdateViewAvailableType0", None]:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                available_type_0 = UpdateViewAvailableType0.from_dict(data)

                return available_type_0
            except:  # noqa: E722
                pass
            return cast(Union["UpdateViewAvailableType0", None], data)

        available = _parse_available(d.pop("available"))

        unit = d.pop("unit")

        may_approve = d.pop("mayApprove")

        approvers = UpdateViewApprovers(d.pop("approvers"))

        policy = UpdateViewPolicy.from_dict(d.pop("policy"))

        pending_approvals = []
        _pending_approvals = d.pop("pendingApprovals")
        for pending_approvals_item_data in _pending_approvals:
            pending_approvals_item = PendingApproval.from_dict(
                pending_approvals_item_data
            )

            pending_approvals.append(pending_approvals_item)

        def _parse_deployment(data: object) -> Union["DeploymentShown", None]:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                deployment_type_0 = DeploymentShown.from_dict(data)

                return deployment_type_0
            except:  # noqa: E722
                pass
            return cast(Union["DeploymentShown", None], data)

        deployment = _parse_deployment(d.pop("deployment"))

        update_view = cls(
            app=app,
            running=running,
            available=available,
            unit=unit,
            may_approve=may_approve,
            approvers=approvers,
            policy=policy,
            pending_approvals=pending_approvals,
            deployment=deployment,
        )

        update_view.additional_properties = d
        return update_view

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
